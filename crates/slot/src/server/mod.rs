//! `slot serve`: queues jobs on the jobserver pool, runs them, streams output
//! to the client and publishes live state to the web UI.

mod http;
mod metrics;
mod ring;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, anyhow, bail};
use nix::sys::signal::{Signal, killpg};
use nix::sys::wait::{Id, WaitPidFlag, waitid};
use nix::unistd::Pid;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader as TokioBufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, watch};

use crate::jobserver::Pool;
use crate::procstat::{self, Proc, Snapshot, Tracker};
use crate::proto::{Frame, Request};
use crate::rewrite::Rewriter;
use crate::toolchain::{self, Context};
use crate::types::{Event, Job, JobState, LiveJob, State, Unit};

pub use ring::Ring;

/// The async runtime the daemon runs on.
pub type Runtime = tokio::runtime::Runtime;

/// A multi-threaded runtime for [`Server::run`].
pub fn runtime() -> Result<Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?)
}

/// Daemon configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Unix socket clients connect to.
    pub socket: PathBuf,
    /// Web UI address; `None` disables it.
    pub http: Option<SocketAddr>,
    /// Pool size.
    pub tokens: usize,
    /// Tokens a go job holds.
    pub go_tokens: usize,
    /// Jobserver FIFO and job log.
    pub state_dir: PathBuf,
    /// Where remote clients sync workspaces.
    pub ws_root: PathBuf,
    /// Target dirs and caches for synced workspaces.
    pub cache_root: PathBuf,
    /// systemd `MemoryMax` per job; `None` = off.
    pub memory_max: Option<String>,
    /// Finished jobs kept in memory.
    pub keep_jobs: usize,
    /// systemd `CPUWeight` for jobs; `None` = leave unset.
    pub cpu_weight: Option<u16>,
}

/// The systemd user unit this process runs in, from `/proc/self/cgroup`.
fn own_unit() -> Option<String> {
    let s = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let path = s.lines().find_map(|l| l.strip_prefix("0::"))?;
    if !path.contains("/user@") {
        return None;
    }
    let last = path.rsplit('/').next()?;
    let ext = Path::new(last).extension().and_then(|e| e.to_str());
    matches!(ext, Some("service" | "scope")).then(|| last.to_string())
}

/// Sets `CPUWeight` on the daemon's own unit so every job, which inherits
/// the cgroup, yields CPU to interactive applications.
fn apply_cpu_weight(weight: u16) -> String {
    let Some(unit) = own_unit() else {
        return "cpu weight not applied: not running in a systemd user unit".into();
    };
    let ok = Command::new("systemctl")
        .args(["--user", "set-property", "--runtime", &unit])
        .arg(format!("CPUWeight={weight}"))
        .stdin(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        format!("cpu weight {weight} on {unit}")
    } else {
        format!("cpu weight not applied: systemctl set-property {unit} failed")
    }
}

/// Environment variable that tags every job with its id. Compile servers
/// such as sccache run the compiler outside the job's process tree but pass
/// the client's environment on, so the tag attributes that CPU to the job.
pub const JOB_ENV: &str = "SLOT_JOB";

/// Process names of compile servers whose children are attributed by tag.
pub const COMPILE_SERVERS: &[&str] = &["sccache"];

/// Work units per job sent in each tick.
const MAX_UNITS: usize = 12;

const OUTPUT_KEEP: usize = 256 << 10;
const DONE_OUTPUT_KEEP: usize = 50;

struct Live {
    job: Job,
    pid: Option<i32>,
    held: usize,
    tracker: Tracker,
    out: Arc<Ring>,
    cores: f64,
    rss: u64,
    procs: usize,
}

struct Done {
    job: Job,
    out: Option<Arc<Ring>>,
}

enum Output {
    NoJob,
    Dropped,
    Kept(Arc<Ring>),
}

struct Inner {
    next_id: i64,
    active: BTreeMap<i64, Live>,
    done: VecDeque<Done>,
    log: File,
    tag_cache: HashMap<i32, i64>,
    sys_prev: (u64, u64),
    sys_cpu: f64,
    job_cores: f64,
    metrics: metrics::Metrics,
}

/// The daemon.
pub struct Server {
    cfg: Config,
    pool: Arc<Pool>,
    inner: Mutex<Inner>,
    events: broadcast::Sender<String>,
    host: String,
    cores: u32,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl Server {
    /// Creates the state directory, the jobserver and loads history.
    pub fn new(cfg: Config) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&cfg.state_dir)
            .with_context(|| format!("create {}", cfg.state_dir.display()))?;
        std::fs::set_permissions(&cfg.state_dir, std::fs::Permissions::from_mode(0o700))?;
        let pool = Pool::new(&cfg.state_dir.join("jobserver.fifo"), cfg.tokens)?;
        let log_path = cfg.state_dir.join("jobs.jsonl");
        let mut done = VecDeque::new();
        let mut next_id = 0;
        if let Ok(f) = File::open(&log_path) {
            for line in BufReader::new(f).lines().map_while(Result::ok) {
                if let Ok(job) = serde_json::from_str::<Job>(&line) {
                    next_id = next_id.max(job.id);
                    done.push_back(Done { job, out: None });
                    if done.len() > cfg.keep_jobs {
                        done.pop_front();
                    }
                }
            }
        }
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("open {}", log_path.display()))?;
        let (events, _) = broadcast::channel(1024);
        let host = nix::unistd::gethostname()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_default();
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
        Ok(Arc::new(Self {
            cfg,
            pool,
            inner: Mutex::new(Inner {
                next_id,
                active: BTreeMap::new(),
                done,
                log,
                tag_cache: HashMap::new(),
                sys_prev: (0, 0),
                sys_cpu: 0.0,
                job_cores: 0.0,
                metrics: metrics::Metrics::default(),
            }),
            events,
            host,
            cores,
        }))
    }

    /// The jobserver pool.
    pub fn pool(&self) -> &Arc<Pool> {
        &self.pool
    }

    /// Serves until the listener fails.
    pub async fn run(self: Arc<Self>) -> Result<()> {
        let _ = std::fs::remove_file(&self.cfg.socket);
        let listener = UnixListener::bind(&self.cfg.socket)
            .with_context(|| format!("bind {}", self.cfg.socket.display()))?;
        std::fs::set_permissions(&self.cfg.socket, std::fs::Permissions::from_mode(0o600))?;
        tokio::spawn(Arc::clone(&self).sampler());
        if let Some(addr) = self.cfg.http {
            let s = Arc::clone(&self);
            tokio::spawn(async move {
                if let Err(e) = http::serve(s, addr).await {
                    eprintln!("slot: http: {e:#}");
                }
            });
        }
        if let Some(w) = self.cfg.cpu_weight {
            let msg = tokio::task::spawn_blocking(move || apply_cpu_weight(w))
                .await
                .unwrap_or_default();
            eprintln!("slot: {msg}");
        }
        eprintln!(
            "slot: socket {}, http {}, {} tokens, fifo {}",
            self.cfg.socket.display(),
            self.cfg
                .http
                .map_or_else(|| "off".to_string(), |a| a.to_string()),
            self.cfg.tokens,
            self.pool.path().display()
        );
        loop {
            let (stream, _) = listener.accept().await?;
            let s = Arc::clone(&self);
            tokio::spawn(async move { s.handle(stream).await });
        }
    }

    async fn handle(self: Arc<Self>, stream: UnixStream) {
        let (rd, mut wr) = stream.into_split();
        let mut rd = TokioBufReader::new(rd);
        let mut line = String::new();
        if rd.read_line(&mut line).await.unwrap_or(0) == 0 {
            return;
        }
        let Ok(req) = serde_json::from_str::<Request>(&line) else {
            return;
        };
        let (gone_tx, gone_rx) = watch::channel(false);
        tokio::spawn(async move {
            let mut sink = [0u8; 256];
            while rd.read(&mut sink).await.unwrap_or(0) > 0 {}
            let _ = gone_tx.send(true);
        });
        let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();
        let writer = tokio::spawn(async move {
            while let Some(f) = rx.recv().await {
                let Ok(mut b) = serde_json::to_vec(&f) else {
                    continue;
                };
                b.push(b'\n');
                if wr.write_all(&b).await.is_err() {
                    break;
                }
                if matches!(f, Frame::Exit { .. }) {
                    break;
                }
            }
            let _ = wr.shutdown().await;
        });
        let (code, msg) = self.run_job(req, tx.clone(), gone_rx).await;
        if let Some(msg) = msg {
            let _ = tx.send(Frame::Info { msg });
        }
        let _ = tx.send(Frame::Exit { code });
        drop(tx);
        let _ = writer.await;
    }

    fn resolve(&self, req: &Request) -> Result<(PathBuf, String, String)> {
        let Some(ws) = &req.workspace else {
            let cwd = PathBuf::from(&req.cwd);
            if !cwd.is_absolute() {
                bail!("cwd must be absolute");
            }
            return Ok((cwd, String::new(), String::new()));
        };
        if ws.id.is_empty() || ws.id.contains(['/', '.']) {
            bail!("bad workspace id");
        }
        let root = self.cfg.ws_root.join(&ws.id);
        let mut cwd = root.clone();
        for c in Path::new(&ws.rel_cwd).components() {
            match c {
                Component::Normal(p) => cwd.push(p),
                Component::ParentDir if cwd != root => {
                    cwd.pop();
                }
                _ => {}
            }
        }
        Ok((
            cwd,
            root.to_string_lossy().into_owned(),
            ws.client_root.clone(),
        ))
    }

    async fn run_job(
        self: &Arc<Self>,
        req: Request,
        tx: mpsc::UnboundedSender<Frame>,
        gone: watch::Receiver<bool>,
    ) -> (i32, Option<String>) {
        if req.argv.is_empty() {
            return (2, Some("slot: empty command".into()));
        }
        let (cwd, from, to) = match self.resolve(&req) {
            Ok(v) => v,
            Err(e) => return (2, Some(format!("slot: {e}"))),
        };
        let tc = toolchain::for_argv(&req.argv, self.cfg.go_tokens);
        let tokens = tc.tokens().clamp(1, self.pool.size());
        let out = Arc::new(Ring::new(OUTPUT_KEEP));
        let id = {
            let mut g = self.inner.lock().expect("server lock");
            g.next_id += 1;
            let id = g.next_id;
            let job = Job {
                id,
                argv: req.argv.clone(),
                cwd: cwd.to_string_lossy().into_owned(),
                repo: req.repo.clone(),
                session: req.session.clone(),
                client: req.client.clone(),
                toolchain: tc.name().to_string(),
                remote: req.workspace.is_some(),
                tokens: tokens as u32,
                state: JobState::Queued,
                enqueued_ms: now_ms(),
                started_ms: 0,
                ended_ms: 0,
                exit: 0,
                cpu_s: 0.0,
                compile_server_cpu_s: 0.0,
                peak_rss: 0,
            };
            g.active.insert(
                id,
                Live {
                    job,
                    pid: None,
                    held: 0,
                    tracker: Tracker::default(),
                    out: Arc::clone(&out),
                    cores: 0.0,
                    rss: 0,
                    procs: 0,
                },
            );
            id
        };
        self.publish(id);

        let ahead = self.pool.waiting();
        let free = self.pool.free();
        if ahead > 0 || free < tokens {
            let _ = tx.send(Frame::Info {
                msg: format!(
                    "slot: job {id} queued for {tokens} slot(s): {free} free, {ahead} request(s) ahead"
                ),
            });
        }
        let mut waiter = self.pool.acquire(tokens);
        tokio::select! {
            () = waiter.ready() => {}
            () = client_gone(gone.clone()) => {
                if self.pool.cancel(&waiter) {
                    self.set_held(id, tokens);
                }
                self.finish(id, JobState::Cancelled, -1, None);
                return (130, None);
            }
        }
        self.set_held(id, tokens);

        let ctx = Context {
            jobserver_auth: self.pool.auth(),
            held: tokens,
            free_tokens: self.pool.free(),
            cache_dir: if req.workspace.is_some() {
                self.cfg.cache_root.to_string_lossy().into_owned()
            } else {
                String::new()
            },
            workspace_id: req
                .workspace
                .as_ref()
                .map(|w| w.id.clone())
                .unwrap_or_default(),
        };
        let mut base = req.env.clone();
        if req.workspace.is_some() {
            // Remote clients send only allowlisted variables; PATH, HOME and
            // the toolchain installs are the server's own.
            base = std::env::vars()
                .map(|(k, v)| format!("{k}={v}"))
                .chain(base)
                .collect();
        }
        let mut env = tc.env(&ctx, base);
        env.push(format!("{JOB_ENV}={id}"));

        let mut argv = req.argv.clone();
        if let Some(mem) = &self.cfg.memory_max {
            let mut wrap: Vec<String> = [
                "systemd-run",
                "--user",
                "--scope",
                "--quiet",
                "--collect",
                "-p",
            ]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
            wrap.push(format!("MemoryMax={mem}"));
            if let Some(w) = self.cfg.cpu_weight {
                wrap.push("-p".into());
                wrap.push(format!("CPUWeight={w}"));
            }
            wrap.push("--".into());
            wrap.extend(argv);
            argv = wrap;
        }
        let program = match look_path(&argv[0], &env) {
            Ok(p) => p,
            Err(e) => {
                self.finish(id, JobState::Failed, 127, None);
                return (127, Some(format!("slot: {e}")));
            }
        };
        let mut cmd = Command::new(&program);
        cmd.arg0(&argv[0])
            .args(&argv[1..])
            .current_dir(&cwd)
            .env_clear()
            .envs(env.iter().filter_map(|e| e.split_once('=')))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.finish(id, JobState::Failed, 127, None);
                return (127, Some(format!("slot: {}: {e}", argv[0])));
            }
        };
        let pid = child.id() as i32;
        {
            let mut g = self.inner.lock().expect("server lock");
            if let Some(l) = g.active.get_mut(&id) {
                l.pid = Some(pid);
                l.job.started_ms = now_ms();
                l.job.state = JobState::Running;
                l.tracker.sample(&Snapshot::scan(), pid, &[], 0.0);
            }
        }
        self.publish(id);

        let pipes: Vec<std::thread::JoinHandle<()>> = [
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .zip(["out", "err"])
        .filter_map(|(p, t)| p.map(|p| (p, t)))
        .map(|(p, t)| {
            let tx = tx.clone();
            let out = Arc::clone(&out);
            let (from, to) = (from.clone(), to.clone());
            std::thread::spawn(move || pump(p, t, &from, &to, &tx, &out))
        })
        .collect();

        let wait = tokio::task::spawn_blocking(move || reap(child));
        tokio::pin!(wait);
        let mut cancelled = false;
        let result = tokio::select! {
            r = &mut wait => r,
            () = client_gone(gone.clone()) => {
                cancelled = true;
                let _ = killpg(Pid::from_raw(pid), Signal::SIGTERM);
                if let Ok(r) = tokio::time::timeout(Duration::from_secs(5), &mut wait).await {
                    r
                } else {
                    let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
                    wait.await
                }
            }
        };
        let (code, cpu) = result.unwrap_or((127, None));
        // A grandchild that keeps a pipe open would block forever: give the
        // readers 2 s after the job's main process exits.
        let deadline = Instant::now() + Duration::from_secs(2);
        for h in pipes {
            while !h.is_finished() && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            if h.is_finished() {
                let _ = h.join();
            }
        }
        if cancelled {
            self.finish(id, JobState::Cancelled, -1, cpu);
            return (130, None);
        }
        let state = if code == 0 {
            JobState::Ok
        } else {
            JobState::Failed
        };
        let job = self.finish(id, state, code, cpu);
        let msg = job.and_then(|j| {
            let wait = j.started_ms - j.enqueued_ms;
            (wait >= 2000).then(|| {
                format!(
                    "slot: job {} waited {}s for a build slot, ran {}s",
                    j.id,
                    wait / 1000,
                    (j.ended_ms - j.started_ms) / 1000
                )
            })
        });
        (code, msg)
    }

    fn set_held(&self, id: i64, n: usize) {
        let mut g = self.inner.lock().expect("server lock");
        if let Some(l) = g.active.get_mut(&id) {
            l.held = n;
        }
    }

    fn finish(&self, id: i64, state: JobState, code: i32, cpu: Option<f64>) -> Option<Job> {
        let (job, held, idle) = {
            let mut g = self.inner.lock().expect("server lock");
            let mut live = g.active.remove(&id)?;
            live.job.state = state;
            live.job.exit = code;
            live.job.ended_ms = now_ms();
            live.job.cpu_s = live.job.compile_server_cpu_s + cpu.unwrap_or(0.0);
            let job = live.job.clone();
            g.metrics.record(&job);
            if let Ok(mut line) = serde_json::to_vec(&job) {
                line.push(b'\n');
                let _ = g.log.write_all(&line);
            }
            g.done.push_back(Done {
                job: job.clone(),
                out: Some(Arc::clone(&live.out)),
            });
            while g.done.len() > self.cfg.keep_jobs {
                g.done.pop_front();
            }
            let n = g.done.len();
            for d in g.done.iter_mut().take(n.saturating_sub(DONE_OUTPUT_KEEP)) {
                d.out = None;
            }
            live.out.close();
            (job, live.held, g.active.is_empty())
        };
        self.pool.release(held);
        self.send(&Event::Job { job: job.clone() });
        if idle {
            let n = self.pool.reconcile();
            if n != 0 {
                eprintln!("slot: jobserver reconciled by {n:+} tokens");
            }
        }
        Some(job)
    }

    fn publish(&self, id: i64) {
        let job = {
            let g = self.inner.lock().expect("server lock");
            g.active.get(&id).map(|l| l.job.clone())
        };
        if let Some(job) = job {
            self.send(&Event::Job { job });
        }
    }

    fn send(&self, e: &Event) {
        if let Ok(s) = serde_json::to_string(e) {
            let _ = self.events.send(s);
        }
    }

    async fn sampler(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut last = Instant::now();
        loop {
            tick.tick().await;
            let dt = last.elapsed().as_secs_f64();
            last = Instant::now();
            let Ok((snap, sys)) =
                tokio::task::spawn_blocking(|| (Snapshot::scan(), procstat::system_cpu())).await
            else {
                continue;
            };
            let event = {
                let mut g = self.inner.lock().expect("server lock");
                let (pb, pt) = g.sys_prev;
                if pt > 0 && sys.1 > pt {
                    g.sys_cpu = (sys.0 - pb) as f64 / (sys.1 - pt) as f64;
                }
                g.sys_prev = sys;
                let extra = compile_server_work(&mut g, &snap);
                let mut jobs = Vec::new();
                let mut total = 0.0;
                for (id, l) in &mut g.active {
                    let Some(pid) = l.pid else { continue };
                    let s =
                        l.tracker
                            .sample(&snap, pid, extra.get(id).map_or(&[], Vec::as_slice), dt);
                    l.cores = s.cores;
                    l.rss = s.rss;
                    l.procs = s.procs;
                    l.job.peak_rss = l.job.peak_rss.max(s.rss);
                    l.job.compile_server_cpu_s += s.extra_ticks as f64 / procstat::CLK_TCK;
                    total += s.cores;
                    jobs.push(LiveJob {
                        id: *id,
                        cores: s.cores,
                        rss: s.rss,
                        procs: s.procs as u32,
                        units: s
                            .units
                            .iter()
                            .take(MAX_UNITS)
                            .map(|u| Unit {
                                kind: u.activity.kind.to_string(),
                                name: u.activity.name.clone(),
                                cores: u.cores,
                                secs: u.secs,
                            })
                            .collect(),
                        units_seen: s.units_seen as u32,
                    });
                }
                g.job_cores = total;
                Event::Tick {
                    t: now_ms(),
                    tokens: self.cfg.tokens as u32,
                    free: self.pool.free() as u32,
                    waiting: self.pool.waiting() as u32,
                    sys_cpu: g.sys_cpu,
                    job_cores: total,
                    jobs,
                }
            };
            self.send(&event);
        }
    }

    /// The `GET /metrics` exposition text.
    pub fn metrics_text(&self) -> String {
        let g = self.inner.lock().expect("server lock");
        let mut gauges = metrics::Gauges {
            tokens: self.cfg.tokens,
            free: self.pool.free(),
            waiting: self.pool.waiting(),
            job_cores: g.job_cores,
            sys_cpu: g.sys_cpu,
            ..metrics::Gauges::default()
        };
        for tc in toolchain::names() {
            gauges.queued.insert(tc.to_string(), 0);
            gauges.running.insert(tc.to_string(), 0);
        }
        for l in g.active.values() {
            let map = if l.job.state == JobState::Queued {
                &mut gauges.queued
            } else {
                &mut gauges.running
            };
            *map.entry(l.job.toolchain.clone()).or_default() += 1;
        }
        g.metrics.render(&gauges)
    }

    /// The `GET /api/state` snapshot.
    pub fn state(&self) -> State {
        let g = self.inner.lock().expect("server lock");
        State {
            host: self.host.clone(),
            cores: self.cores,
            tokens: self.cfg.tokens as u32,
            free: self.pool.free() as u32,
            active: g.active.values().map(|l| l.job.clone()).collect(),
            done: g.done.iter().map(|d| d.job.clone()).collect(),
            now_ms: now_ms(),
            toolchains: toolchain::names().into_iter().map(String::from).collect(),
        }
    }

    fn output(&self, id: i64) -> Output {
        let g = self.inner.lock().expect("server lock");
        if let Some(l) = g.active.get(&id) {
            return Output::Kept(Arc::clone(&l.out));
        }
        match g.done.iter().rev().find(|d| d.job.id == id) {
            None => Output::NoJob,
            Some(Done { out: Some(r), .. }) => Output::Kept(Arc::clone(r)),
            Some(_) => Output::Dropped,
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<String> {
        self.events.subscribe()
    }
}

/// Resolves when the client has gone away.
async fn client_gone(mut rx: watch::Receiver<bool>) {
    let _ = rx.wait_for(|g| *g).await.map(|_| ());
}

fn compile_server_work(g: &mut Inner, snap: &Snapshot) -> HashMap<i64, Vec<Proc>> {
    let mut in_job = HashSet::new();
    for l in g.active.values() {
        if let Some(pid) = l.pid {
            in_job.extend(snap.tree(pid).iter().map(|p| p.pid));
        }
    }
    let mut seen = HashSet::new();
    let mut out: HashMap<i64, Vec<Proc>> = HashMap::new();
    for name in COMPILE_SERVERS {
        for d in snap.daemons(name, &in_job) {
            for p in snap.tree(d) {
                if p.pid == d {
                    continue;
                }
                seen.insert(p.pid);
                let id = *g.tag_cache.entry(p.pid).or_insert_with(|| {
                    procstat::env_value(p.pid, JOB_ENV)
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0)
                });
                if id != 0 {
                    out.entry(id).or_default().push(p);
                }
            }
        }
    }
    g.tag_cache.retain(|pid, _| seen.contains(pid));
    out
}

fn pump(
    mut p: Box<dyn Read + Send>,
    t: &str,
    from: &str,
    to: &str,
    tx: &mpsc::UnboundedSender<Frame>,
    out: &Ring,
) {
    let mut rw = Rewriter::new(from, to);
    let mut buf = vec![0u8; 32 << 10];
    let emit = |b: Vec<u8>| {
        if b.is_empty() {
            return;
        }
        let s = String::from_utf8_lossy(&b).into_owned();
        out.write(t, s.clone());
        let f = if t == "out" {
            Frame::Out { b: s }
        } else {
            Frame::Err { b: s }
        };
        let _ = tx.send(f);
    };
    loop {
        match p.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => emit(rw.push(&buf[..n])),
        }
    }
    emit(rw.finish());
}

/// Waits for the child, reads its CPU time (including reaped descendants)
/// while it is still a zombie, then reaps it.
fn reap(mut child: std::process::Child) -> (i32, Option<f64>) {
    let pid = Pid::from_raw(child.id() as i32);
    let _ = waitid(Id::Pid(pid), WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT);
    let cpu = procstat::total_cpu_seconds(pid.as_raw());
    let code = match child.wait() {
        Ok(st) => st.code().unwrap_or_else(|| 128 + st.signal().unwrap_or(0)),
        Err(_) => 127,
    };
    (code, cpu)
}

fn look_path(name: &str, env: &[String]) -> Result<PathBuf> {
    if name.contains('/') {
        return Ok(PathBuf::from(name));
    }
    let path = toolchain::get(env, "PATH")
        .map(String::from)
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let f = dir.join(name);
        if let Ok(m) = std::fs::metadata(&f)
            && m.is_file()
            && m.permissions().mode() & 0o111 != 0
        {
            return Ok(f);
        }
    }
    Err(anyhow!("{name}: not found in PATH"))
}

#[cfg(test)]
mod tests;
