//! `slot serve`: queues jobs on the jobserver pool, runs them, streams output
//! to the client and publishes live state to the web UI.

pub mod cache;
pub mod handover;
mod http;
pub mod limits;
mod metrics;
mod ring;
mod upgrade;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::SocketAddr;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, anyhow, bail};
use nix::poll::{PollFd, PollFlags, poll};
use nix::sys::signal::{Signal, killpg};
use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid, waitpid};
use nix::unistd::Pid;
use tokio::io::{
    AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader as TokioBufReader,
};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::admission;
use crate::call;
use crate::census;
use crate::jobserver::{self, Pool, WaiterHandle};
use crate::policy;
use crate::procstat::{self, Proc, Snapshot, Tracker};
use crate::proto::{Frame, Request};
use crate::rewrite::Rewriter;
use crate::toolchain::{self, Context};
use crate::types::{Event, HostCensus, Job, JobState, LiveJob, SccacheWatch, State, Unit};

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
    /// Whether a headroom also sets `CPUQuota` on the daemon's systemd unit.
    pub cpu_quota: bool,
    /// Default thread count of test runners in a job (`RUST_TEST_THREADS`,
    /// `NEXTEST_TEST_THREADS`, `GOMAXPROCS`) when the caller sets none.
    pub test_threads: usize,
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
    /// Call mode: headroom while a call holds the microphone or the camera,
    /// percent; 0 = off. A value saved through `PUT /api/limits` wins.
    pub call_headroom: u8,
    /// Time without capture after which a call counts as ended.
    pub call_off_after: Duration,
    /// Applications whose capture is not a call (level meters, mixers).
    pub call_ignore: Vec<String>,
    /// Watch sccache servers and give back jobserver tokens they lost.
    pub sccache_watch: bool,
    /// Free memory and disk a job needs before it may start; 0 turns one off.
    pub admission: admission::Limits,
    /// Longest time an upgrade stops admission while jobs run; zero never stops it.
    pub upgrade_hold: Duration,
    /// Bytes the target directories of synced workspaces may take together
    /// before the least recently used are deleted; 0 = no budget (the disk
    /// floor still evicts).
    pub cache_budget: u64,
    /// Eviction of local target directories under a disk floor.
    pub evict: crate::evict::Settings,
    /// On reload, hand running and queued jobs to the new binary instead of
    /// waiting for an idle pool (`handover`); the bounded hold is the fallback.
    pub handover: bool,
}

/// systemd slice the scopes of jobs under `--memory-max` run in; the
/// headroom's CPU quota is set on it too.
pub const JOB_SLICE: &str = "slot.slice";

/// How often a job that waits for disk or memory looks again.
const RESOURCE_POLL: Duration = Duration::from_secs(2);

/// Jobs that started this recently count as still growing towards their
/// expected peak memory.
const STARTING_MS: i64 = 30_000;

/// Interval between two call probes.
const CALL_PROBE: Duration = Duration::from_secs(5);

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

/// Samples averaged into one census log line (one per second).
const CENSUS_WINDOW: u32 = 10;

/// Ticks kept for `GET /api/history` (one per second).
const HISTORY_TICKS: usize = 900;

/// Work units per job sent in each tick.
const MAX_UNITS: usize = 12;

const OUTPUT_KEEP: usize = 256 << 10;
const DONE_OUTPUT_KEEP: usize = 50;

/// How often [`Server::token_watch`] looks for lost jobserver tokens.
const TOKEN_WATCH: Duration = Duration::from_secs(15);

struct Live {
    job: Job,
    pid: Option<i32>,
    held: usize,
    tracker: Tracker,
    out: Arc<Ring>,
    cores: f64,
    rss: u64,
    procs: usize,
    /// Peak memory the job is expected to reach, from earlier runs.
    expect_rss: u64,
    /// How `slot cancel` stops the job.
    stop: Option<Stop>,
    /// The request, while queued: a handover queues it again.
    req: Option<Request>,
    /// The queued request's tokens, which a handover takes back.
    waiter: Option<WaiterHandle>,
    /// Output pump threads of the running job.
    pumps: Vec<std::thread::JoinHandle<PumpEnd>>,
    /// Path rewrite of a remote job: from, to.
    rewrite: (String, String),
    /// Exit code and CPU seconds once reaped.
    exited: Option<handover::Exited>,
    /// The client connection.
    conn: Option<u64>,
}

/// A pump thread's end: its stream, and the pipe when it stopped for a
/// handover before the end of the stream.
type PumpEnd = (&'static str, Option<OwnedFd>);

/// A client connection, for a handover.
struct ConnEntry {
    /// A duplicate of the socket, which outlives the tokio halves across exec.
    fd: OwnedFd,
    uid: Option<u32>,
    /// Asks the writer for its unsent bytes; it writes nothing afterwards.
    ctl: Ctl,
}

/// Sends a client writer a request for its unsent bytes.
type Ctl = mpsc::UnboundedSender<oneshot::Sender<Vec<u8>>>;

/// The job record was not finished: a handover took it.
#[derive(Debug)]
struct HandedOver;

/// The submitter of a job and the means to stop it.
#[derive(Clone)]
struct Stop {
    /// User id of the client that submitted the job; `None` when unknown.
    uid: Option<u32>,
    /// Set to `true` to cancel, as when the client goes away.
    gone: Arc<watch::Sender<bool>>,
    /// The submitting client's frames.
    frames: mpsc::UnboundedSender<Frame>,
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
    census: census::Census,
    window: census::Window,
    census_log: File,
    titles: HashMap<String, String>,
    title_pending: HashSet<String>,
    last_census: HostCensus,
    controller: policy::Controller,
    /// Peak memory of finished jobs, for admission.
    memory: admission::History,
    /// Set once a handover took the job table: nothing finishes, reaps,
    /// starts or answers a client after it in this process.
    handing_over: bool,
    /// Local target directories jobs built into: repository and last build,
    /// Unix milliseconds. Eviction only ever deletes one of these.
    targets: HashMap<PathBuf, (String, i64)>,
}

/// The daemon.
pub struct Server {
    cfg: Config,
    pool: Arc<Pool>,
    inner: Mutex<Inner>,
    events: broadcast::Sender<String>,
    history: Mutex<VecDeque<String>>,
    host: String,
    cores: u32,
    headroom: Mutex<Headroom>,
    /// True while an upgrade holds admission: no job is admitted.
    upgrading: watch::Sender<bool>,
    /// A pending upgrade and its admission hold.
    upgrade: Mutex<upgrade::Plan>,
    sccache: Mutex<SccacheWatch>,
    /// Open client connections.
    conns: Mutex<HashMap<u64, ConnEntry>>,
    next_conn: AtomicU64,
    /// Tells every output pump to stop between two reads.
    pause_pumps: Arc<AtomicBool>,
    /// What a handover passed to this process, until `run` takes it.
    inherited: Mutex<Option<handover::Inherited>>,
    /// Lost jobserver tokens written back since start.
    restored: AtomicU64,
    /// What eviction of target directories did last.
    evict: Mutex<crate::evict::Report>,
    /// Target directories being deleted now: their jobs wait.
    evicting: Mutex<HashSet<PathBuf>>,
}

/// The headroom in force, whether its CPU quota is applied, and call mode.
#[derive(Debug, Clone, Default)]
struct Headroom {
    percent: u8,
    quota: Option<u32>,
    note: String,
    /// Headroom during a call; 0 = call mode off.
    call_percent: u8,
    /// `call_percent` as saved through the API; `None` = the daemon default.
    call_saved: Option<u8>,
    /// The headroom last saved, which a restart starts from.
    saved_percent: u8,
    call: call::Tracker,
    call_note: String,
}

/// Slots a job takes. The client's `--tokens` replaces the toolchain's
/// count and is capped at `limit`, the tokens in circulation, so the job
/// can always start; the toolchain's count is capped at the pool `size`
/// (the jobserver caps a request at the limit when it queues).
fn job_tokens(toolchain: usize, requested: Option<u32>, size: usize, limit: usize) -> usize {
    match requested {
        Some(n) => (n as usize).clamp(1, limit.max(1)),
        None => toolchain.clamp(1, size),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl Server {
    /// Creates the state directory, the jobserver and loads history.
    pub fn new(cfg: Config) -> Result<Arc<Self>> {
        Self::with_inherited(cfg, None)
    }

    /// Like [`Server::new`], taking over what a handover passed on
    /// ([`handover::take_inherited`]): the jobserver FIFO is adopted, not
    /// recreated, and `run` resumes the jobs.
    pub fn with_inherited(
        cfg: Config,
        mut inherited: Option<handover::Inherited>,
    ) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&cfg.state_dir)
            .with_context(|| format!("create {}", cfg.state_dir.display()))?;
        std::fs::set_permissions(&cfg.state_dir, std::fs::Permissions::from_mode(0o700))?;
        let fifo = cfg.state_dir.join("jobserver.fifo");
        let saved = limits::load(&cfg.state_dir);
        let adopted = inherited.as_mut().and_then(|h| {
            if h.state.pool.size != cfg.tokens {
                eprintln!(
                    "slot: handover pool has {} tokens, this daemon {}: not adopting it",
                    h.state.pool.size, cfg.tokens
                );
                return None;
            }
            let held = h.take(Some(h.state.fifo));
            match Pool::adopt(&fifo, h.state.pool, held) {
                Ok(p) => Some(p),
                Err(e) => {
                    eprintln!("slot: handover: {e:#}");
                    None
                }
            }
        });
        let pool = if let Some(p) = adopted {
            p
        } else {
            if let Some(h) = inherited.take() {
                handover::abandon(h);
            }
            Pool::new(&fifo, cfg.tokens)?
        };
        pool.set_limit(limits::effective_tokens(cfg.tokens, saved.headroom_percent));
        let log_path = cfg.state_dir.join("jobs.jsonl");
        let mut done = VecDeque::new();
        let mut next_id = 0;
        let mut memory = admission::History::default();
        let mut targets = HashMap::new();
        if let Ok(f) = File::open(&log_path) {
            for line in BufReader::new(f).lines().map_while(Result::ok) {
                if let Ok(job) = serde_json::from_str::<Job>(&line) {
                    next_id = next_id.max(job.id);
                    memory.record(&job);
                    record_target(&mut targets, &job);
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
        let census_path = cfg.state_dir.join("census.jsonl");
        let census_log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&census_path)
            .with_context(|| format!("open {}", census_path.display()))?;
        let (events, _) = broadcast::channel(1024);
        let host = nix::unistd::gethostname()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_default();
        if let Some(h) = &inherited {
            next_id = next_id.max(h.state.next_id);
        }
        let cores = procstat::host_cpus() as u32;
        let pool_size = cfg.tokens;
        let call_headroom = cfg.call_headroom;
        let sccache_watch = cfg.sccache_watch;
        let upgrade_hold = cfg.upgrade_hold;
        let evict_settings = cfg.evict;
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
                census: census::Census::default(),
                window: census::Window::default(),
                census_log,
                titles: HashMap::new(),
                title_pending: HashSet::new(),
                last_census: HostCensus::default(),
                controller: policy::Controller::new(policy::Settings::new(
                    cores as usize,
                    pool_size,
                )),
                memory,
                handing_over: false,
                targets,
            }),
            events,
            history: Mutex::new(VecDeque::new()),
            host,
            cores,
            headroom: Mutex::new(Headroom {
                percent: saved.headroom_percent,
                saved_percent: saved.headroom_percent,
                call_percent: saved
                    .call_headroom_percent
                    .unwrap_or(call_headroom)
                    .min(limits::MAX_HEADROOM),
                call_saved: saved.call_headroom_percent,
                ..Headroom::default()
            }),
            upgrading: watch::Sender::new(false),
            upgrade: Mutex::new(upgrade::Plan::new(upgrade_hold)),
            sccache: Mutex::new(SccacheWatch {
                enabled: sccache_watch,
                ..SccacheWatch::default()
            }),
            conns: Mutex::new(HashMap::new()),
            next_conn: AtomicU64::new(1),
            pause_pumps: Arc::new(AtomicBool::new(false)),
            inherited: Mutex::new(inherited),
            restored: AtomicU64::new(0),
            evict: Mutex::new(crate::evict::Report {
                floor: evict_settings.floor,
                dry_run: evict_settings.dry_run,
                ..crate::evict::Report::default()
            }),
            evicting: Mutex::new(HashSet::new()),
        }))
    }

    /// Stops admitting jobs: queued ones stay queued, new ones queue. Set
    /// when an upgrade is requested, so the running jobs can end and the
    /// daemon re-executes; the queued clients resubmit to the new process.
    pub(crate) fn hold_admission(&self) {
        self.upgrading.send_replace(true);
    }

    /// Admits jobs again after a hold that ran out.
    fn open_admission(&self) {
        self.upgrading.send_replace(false);
    }

    /// The longest predicted time left of the running jobs; `None` when one
    /// has no prediction or has run past it.
    fn predicted_remaining(&self) -> Option<Duration> {
        let g = self.inner.lock().expect("server lock");
        let now = now_ms();
        let mut longest = 0u64;
        for l in g.active.values() {
            if l.pid.is_none() && l.held == 0 {
                continue;
            }
            let expected = g.memory.expected_runtime(&l.job.repo, &l.job.argv)?;
            let elapsed = if l.job.started_ms > 0 {
                (now - l.job.started_ms).max(0) as u64
            } else {
                0
            };
            if elapsed >= expected {
                return None;
            }
            longest = longest.max(expected - elapsed);
        }
        Some(Duration::from_millis(longest))
    }

    /// Carries out an upgrade step; `true` when it is time to re-execute.
    fn upgrade_step(&self, step: upgrade::Step) -> bool {
        match step {
            upgrade::Step::Reexec => return true,
            upgrade::Step::Hold => {
                self.hold_admission();
                eprintln!(
                    "slot: upgrade pending; admitting no new job for up to {}s",
                    self.cfg.upgrade_hold.as_secs()
                );
            }
            upgrade::Step::Release => {
                let running = {
                    let g = self.inner.lock().expect("server lock");
                    g.active.values().filter(|l| l.pid.is_some()).count()
                };
                self.open_admission();
                eprintln!(
                    "slot: upgrade pending; {running} job(s) still running after {}s, admitting jobs \
                     again; holding again once they are expected to end within {}s or none runs",
                    self.cfg.upgrade_hold.as_secs(),
                    self.cfg.upgrade_hold.as_secs()
                );
            }
            upgrade::Step::Wait => {}
        }
        false
    }

    /// Waits until admission is open; false when the client went away first.
    async fn admission_open(&self, gone: watch::Receiver<bool>) -> bool {
        let mut open = self.upgrading.subscribe();
        tokio::select! {
            r = open.wait_for(|held| !held) => r.is_ok(),
            () = client_gone(gone) => false,
        }
    }

    /// Whether no job holds slots or runs a process: the moment to re-execute.
    fn nothing_running(&self) -> bool {
        let g = self.inner.lock().expect("server lock");
        g.active.values().all(|l| l.pid.is_none() && l.held == 0)
    }

    /// The jobserver pool.
    pub fn pool(&self) -> &Arc<Pool> {
        &self.pool
    }

    /// Serves until the listener fails.
    pub async fn run(self: Arc<Self>) -> Result<()> {
        let inherited = self.inherited.lock().expect("inherited lock").take();
        let (listener, inherited) = if let Some(mut h) = inherited {
            let fd = h.take(Some(h.state.listener));
            if let Some(Ok(l)) = fd.map(|fd| {
                let l = std::os::unix::net::UnixListener::from(fd);
                l.set_nonblocking(true)?;
                UnixListener::from_std(l)
            }) {
                (l, Some(h))
            } else {
                eprintln!("slot: handover: listening socket not inherited, binding anew");
                (self.bind()?, Some(h))
            }
        } else {
            (self.bind()?, None)
        };
        let mut reload_now = false;
        if let Some(h) = inherited {
            reload_now = h.state.reload;
            self.resume(h);
        }
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
        tokio::spawn(Arc::clone(&self).call_watch());
        tokio::spawn(Arc::clone(&self).token_watch());
        if self.cfg.evict.floor > 0 {
            tokio::spawn(Arc::clone(&self).evict_watch());
        }
        if self.cfg.sccache_watch {
            tokio::spawn(Arc::clone(&self).sccache_watch());
        }
        let h = self.headroom_percent();
        if h > 0 {
            let v = self.apply_headroom(h, false).await;
            eprintln!(
                "slot: headroom {h}%: {} of {} tokens, cpu quota {}",
                v.effective_tokens,
                v.tokens,
                v.cpu_quota_percent
                    .map_or_else(|| format!("not applied ({})", v.note), |q| format!("{q}%"))
            );
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
        let mut term = signal(SignalKind::terminate())?;
        let mut hup = signal(SignalKind::hangup())?;
        // A SIGHUP that came while this image started, or while the old one
        // handed over, is acted on now.
        reload_now |= handover::take_hup();
        let mut idle_check = tokio::time::interval(Duration::from_millis(500));
        let mut upgrade = false;
        loop {
            let reload = tokio::select! {
                r = listener.accept() => {
                    let (stream, _) = r?;
                    let s = Arc::clone(&self);
                    tokio::spawn(async move { s.handle(stream).await });
                    false
                }
                _ = term.recv() => break,
                _ = hup.recv() => true,
                () = std::future::ready(()), if reload_now => true,
                _ = idle_check.tick(), if upgrade => {
                    let idle = self.nothing_running();
                    let remaining = if idle { None } else { self.predicted_remaining() };
                    let step = self
                        .upgrade
                        .lock()
                        .expect("upgrade lock")
                        .step(Instant::now(), idle, remaining);
                    if self.upgrade_step(step) {
                        drop(listener);
                        return Err(self.reexec());
                    }
                    false
                }
            };
            if reload {
                reload_now = false;
                handover::take_hup();
                if self.cfg.handover {
                    match self.hand_over(&listener).await {
                        Ok(never) => match never {},
                        Err(Abort::Fatal(e)) => return Err(e),
                        Err(Abort::Retry(why)) => {
                            eprintln!(
                                "slot: handover not possible ({why}); using the bounded hold"
                            );
                        }
                    }
                }
                upgrade = true;
                eprintln!("slot: upgrade requested; re-executing once no job runs");
                let idle = self.nothing_running();
                let step = self
                    .upgrade
                    .lock()
                    .expect("upgrade lock")
                    .request(Instant::now(), idle);
                if self.upgrade_step(step) {
                    drop(listener);
                    return Err(self.reexec());
                }
            }
        }
        self.drain(listener).await;
        Ok(())
    }

    /// Replaces this process with the binary now installed at our path,
    /// same pid and arguments. Called only while no job runs, so the new
    /// process starts with a full pool and reloads history from the job log.
    /// Queued jobs are dropped with their connections; their clients
    /// resubmit. Returns only on failure.
    fn reexec(&self) -> anyhow::Error {
        let exe = installed_exe();
        let args: Vec<String> = std::env::args().skip(1).collect();
        let _ = std::fs::remove_file(&self.cfg.socket);
        let _ = handover::ignore_hup();
        eprintln!("slot: idle, re-executing {}", exe.display());
        let err = Command::new(&exe)
            .args(&args)
            .env_remove(handover::ENV)
            .exec();
        anyhow!("re-exec {}: {err}", exe.display())
    }

    fn bind(&self) -> Result<UnixListener> {
        let _ = std::fs::remove_file(&self.cfg.socket);
        let listener = UnixListener::bind(&self.cfg.socket)
            .with_context(|| format!("bind {}", self.cfg.socket.display()))?;
        std::fs::set_permissions(&self.cfg.socket, std::fs::Permissions::from_mode(0o600))?;
        Ok(listener)
    }

    /// Re-executes the installed binary now and hands it every running and
    /// queued job (`docs/design.md` § 15.2). `Retry` when nothing was
    /// changed for good and the bounded hold can take over; `Fatal` when the
    /// job table was already given up and only exiting is left.
    async fn hand_over(
        self: &Arc<Self>,
        listener: &UnixListener,
    ) -> Result<std::convert::Infallible, Abort> {
        let exe = installed_exe();
        let check = exe.clone();
        let ok = tokio::task::spawn_blocking(move || handover::supports(&check))
            .await
            .unwrap_or(false);
        if !ok {
            return Err(Abort::Retry(format!(
                "{} is older than {}.{}.{} or does not run",
                exe.display(),
                handover::SINCE.0,
                handover::SINCE.1,
                handover::SINCE.2
            )));
        }
        let started = Instant::now();
        let was_held = *self.upgrading.borrow();
        self.hold_admission();
        let undo = |why: String| {
            self.pool.resume();
            if !was_held {
                self.open_admission();
            }
            Abort::Retry(why)
        };
        let pool = Arc::clone(&self.pool);
        let paused = tokio::task::spawn_blocking(move || pool.pause(Duration::from_secs(2)))
            .await
            .unwrap_or(false);
        if !paused {
            return Err(undo("the jobserver reader did not stop within 2 s".into()));
        }
        // Jobs granted their slots before admission stopped are starting:
        // wait until each has its process, at most 2 s.
        let deadline = Instant::now() + Duration::from_secs(2);
        let taken = loop {
            {
                let mut g = self.inner.lock().expect("server lock");
                if !g.active.values().any(|l| l.held > 0 && l.pid.is_none()) {
                    g.handing_over = true;
                    break self.take_table(&mut g);
                }
            }
            if Instant::now() >= deadline {
                return Err(undo(
                    "a job that got its slots did not start within 2 s".into(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        // From here on this process only hands over.
        self.pause_pumps.store(true, Ordering::SeqCst);
        let Table {
            mut running,
            queued,
            pumps,
            next_id,
        } = taken;
        let ends = tokio::task::spawn_blocking(move || {
            pumps
                .into_iter()
                .map(|hs| {
                    hs.into_iter()
                        .filter_map(|h| h.join().ok())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| Abort::Fatal(anyhow!("join output pumps: {e}")))?;
        let mut keep: Vec<OwnedFd> = Vec::new();
        for ((r, _), ends) in running.iter_mut().zip(ends) {
            for (t, fd) in ends {
                let n = fd.as_ref().map(handover::raw);
                if t == "out" {
                    r.stdout = n;
                } else {
                    r.stderr = n;
                }
                keep.extend(fd);
            }
        }
        let conns: Vec<(u64, OwnedFd, Option<u32>, Ctl)> = {
            let g = self.conns.lock().expect("conns lock");
            g.iter()
                .filter_map(|(id, c)| {
                    c.fd.try_clone()
                        .ok()
                        .map(|fd| (*id, fd, c.uid, c.ctl.clone()))
                })
                .collect()
        };
        let mut state = handover::State {
            version: handover::VERSION,
            listener: handover::raw(listener),
            fifo: handover::raw(self.pool.file()),
            pool: self.pool.state(),
            next_id,
            ..handover::State::default()
        };
        let mut index: HashMap<u64, usize> = HashMap::new();
        for (id, fd, uid, ctl) in conns {
            let (tx, rx) = oneshot::channel();
            if ctl.send(tx).is_err() {
                continue;
            }
            let Ok(Ok(pending)) = tokio::time::timeout(Duration::from_secs(1), rx).await else {
                continue;
            };
            index.insert(id, state.conns.len());
            state.conns.push(handover::Conn {
                fd: handover::raw(&fd),
                uid,
                pending,
            });
            keep.push(fd);
        }
        for (r, conn) in &mut running {
            r.conn = conn.and_then(|c| index.get(&c).copied());
        }
        state.running = running.into_iter().map(|(r, _)| r).collect();
        state.queued = queued
            .into_iter()
            .map(|(mut q, conn)| {
                q.conn = conn.and_then(|c| index.get(&c).copied());
                q
            })
            .collect();
        let fatal = |e: anyhow::Error| Abort::Fatal(e);
        handover::keep_on_exec(listener.as_fd()).map_err(fatal)?;
        handover::keep_on_exec(self.pool.file().as_fd()).map_err(fatal)?;
        for fd in &keep {
            handover::keep_on_exec(fd.as_fd()).map_err(fatal)?;
        }
        handover::ignore_hup().map_err(fatal)?;
        state.reload = handover::take_hup();
        let path = handover::write(&self.cfg.state_dir, &state).map_err(fatal)?;
        eprintln!(
            "slot: handing {} running and {} queued job(s) and {} connection(s) to {} after {} ms",
            state.running.len(),
            state.queued.len(),
            state.conns.len(),
            exe.display(),
            started.elapsed().as_millis()
        );
        let args: Vec<String> = std::env::args().skip(1).collect();
        let err = Command::new(&exe)
            .args(&args)
            .env(handover::ENV, &path)
            .exec();
        Err(Abort::Fatal(anyhow!("re-exec {}: {err}", exe.display())))
    }

    /// Takes the job table for a handover, under the server lock: running
    /// jobs with their pumps, queued jobs with their requests. Tokens that
    /// queued requests gathered go back to the FIFO.
    fn take_table(&self, g: &mut Inner) -> Table {
        let mut t = Table {
            next_id: g.next_id,
            ..Table::default()
        };
        for (id, l) in &mut g.active {
            if let Some(pid) = l.pid {
                t.pumps.push(std::mem::take(&mut l.pumps));
                t.running.push((
                    handover::Running {
                        job: l.job.clone(),
                        pid,
                        held: l.held,
                        stdout: None,
                        stderr: None,
                        rewrite: l.rewrite.clone(),
                        exited: l.exited,
                        conn: None,
                    },
                    l.conn,
                ));
            } else {
                if let Some(w) = l.waiter.take() {
                    self.pool.release(w.take());
                }
                if let Some(req) = l.req.clone() {
                    t.queued.push((
                        handover::Queued {
                            id: *id,
                            enqueued_ms: l.job.enqueued_ms,
                            req,
                            conn: None,
                        },
                        l.conn,
                    ));
                }
            }
        }
        t
    }

    /// Takes over what a handover passed on: client connections, running
    /// jobs (their pumps restart, their pid is reaped here) and queued jobs.
    fn resume(self: &Arc<Self>, mut h: handover::Inherited) {
        let state = std::mem::take(&mut h.state);
        let mut conns: Vec<Option<Attached>> = Vec::new();
        for c in state.conns {
            let a = h.take(Some(c.fd)).and_then(|fd| {
                let dup = fd.try_clone().ok()?;
                let s = std::os::unix::net::UnixStream::from(fd);
                s.set_nonblocking(true).ok()?;
                let s = UnixStream::from_std(s).ok()?;
                let (rd, wr) = s.into_split();
                Some(self.attach(rd, wr, dup, c.uid, c.pending))
            });
            conns.push(a);
        }
        let n_running = state.running.len();
        let snap = Snapshot::scan();
        for r in state.running {
            let conn = r.conn.and_then(|i| conns.get_mut(i).and_then(Option::take));
            let out = h.take(r.stdout);
            let err = h.take(r.stderr);
            let (id, pid) = (r.job.id, r.pid);
            // Into the table before the first client is accepted, so an idle
            // reconcile never takes the tokens these jobs hold.
            let (tx, gone, keep) = self.adopt_running(r, out, err, conn.as_ref(), &snap);
            let s = Arc::clone(self);
            tokio::spawn(async move {
                let _keep = keep;
                let done = s.supervise(id, pid, &tx, gone).await;
                drop(tx);
                s.end_conn(conn, done.is_some()).await;
            });
        }
        let n_queued = state.queued.len();
        for q in state.queued {
            let conn = q.conn.and_then(|i| conns.get_mut(i).and_then(Option::take));
            let s = Arc::clone(self);
            tokio::spawn(async move { s.adopt_queued(q, conn).await });
        }
        // Connections of jobs that had ended: send what was left, then close.
        for a in conns.into_iter().flatten() {
            let Attached { id, writer, tx, .. } = a;
            drop(tx);
            let s = Arc::clone(self);
            tokio::spawn(async move {
                let _ = writer.await;
                s.conns.lock().expect("conns lock").remove(&id);
            });
        }
        eprintln!("slot: took over {n_running} running and {n_queued} queued job(s)");
    }

    /// Puts a job a handover passed on into the table and restarts its
    /// pumps. Returns the frame sender, the gone signal and, for a job
    /// whose client is gone, the signal's sender to keep alive.
    fn adopt_running(
        &self,
        r: handover::Running,
        out: Option<OwnedFd>,
        err: Option<OwnedFd>,
        conn: Option<&Attached>,
        snap: &Snapshot,
    ) -> (
        mpsc::UnboundedSender<Frame>,
        watch::Receiver<bool>,
        Option<watch::Sender<bool>>,
    ) {
        let id = r.job.id;
        let (tx, gone, stop, keep) = if let Some(a) = conn {
            (a.tx.clone(), a.gone_rx.clone(), Some(a.stop()), None)
        } else {
            let (tx, _rx) = mpsc::unbounded_channel();
            let (g, rx) = watch::channel(false);
            (tx, rx, None, Some(g))
        };
        let ring = Arc::new(Ring::new(OUTPUT_KEEP));
        let pumps = self.start_pumps([out, err], &tx, &ring, &r.rewrite);
        {
            let mut g = self.inner.lock().expect("server lock");
            let expect_rss = g
                .memory
                .expected(&r.job.repo, &r.job.argv, &r.job.toolchain);
            let mut tracker = Tracker::default();
            tracker.sample(snap, r.pid, &[], 0.0);
            g.active.insert(
                id,
                Live {
                    job: r.job,
                    pid: Some(r.pid),
                    held: r.held,
                    tracker,
                    out: ring,
                    cores: 0.0,
                    rss: 0,
                    procs: 0,
                    expect_rss,
                    stop,
                    req: None,
                    waiter: None,
                    pumps,
                    rewrite: r.rewrite,
                    exited: r.exited,
                    conn: conn.map(|a| a.id),
                },
            );
        }
        self.publish(id);
        (tx, gone, keep)
    }

    async fn adopt_queued(self: Arc<Self>, q: handover::Queued, conn: Option<Attached>) {
        let Some(a) = conn else {
            // Nobody waits for it any more.
            return;
        };
        let stop = a.stop();
        let done = self
            .run_job(
                q.req,
                a.tx.clone(),
                a.gone_rx.clone(),
                Some(stop),
                Some(a.id),
                Some((q.id, q.enqueued_ms)),
            )
            .await;
        self.end_conn(Some(a), done.is_some()).await;
    }

    /// Ends a connection after its job: waits for the writer to send the
    /// exit frame and forgets the connection. When a handover took the job,
    /// waits for the re-exec instead.
    async fn end_conn(&self, a: Option<Attached>, done: bool) {
        if !done {
            std::future::pending::<()>().await;
        }
        if let Some(a) = a {
            let id = a.id;
            drop(a.tx);
            let _ = a.writer.await;
            self.conns.lock().expect("conns lock").remove(&id);
        }
    }

    /// Registers a client connection: a reader that notices the client
    /// leave, a writer that sends `pending` and then frames.
    fn attach<R>(
        self: &Arc<Self>,
        rd: R,
        wr: OwnedWriteHalf,
        dup: OwnedFd,
        uid: Option<u32>,
        pending: Vec<u8>,
    ) -> Attached
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let id = self.next_conn.fetch_add(1, Ordering::Relaxed);
        let (gone_tx, gone_rx) = watch::channel(false);
        let gone_tx = Arc::new(gone_tx);
        let reader_gone = Arc::clone(&gone_tx);
        tokio::spawn(async move {
            let mut rd = rd;
            let mut sink = [0u8; 256];
            while rd.read(&mut sink).await.unwrap_or(0) > 0 {}
            reader_gone.send_replace(true);
        });
        let (tx, rx) = mpsc::unbounded_channel::<Frame>();
        let (ctl_tx, ctl_rx) = mpsc::unbounded_channel();
        let writer = tokio::spawn(client_writer(wr, rx, ctl_rx, pending));
        self.conns.lock().expect("conns lock").insert(
            id,
            ConnEntry {
                fd: dup,
                uid,
                ctl: ctl_tx,
            },
        );
        Attached {
            id,
            uid,
            tx,
            gone_tx,
            gone_rx,
            writer,
        }
    }

    /// Starts one pump thread per open pipe.
    fn start_pumps(
        &self,
        fds: [Option<OwnedFd>; 2],
        tx: &mpsc::UnboundedSender<Frame>,
        out: &Arc<Ring>,
        rewrite: &(String, String),
    ) -> Vec<std::thread::JoinHandle<PumpEnd>> {
        fds.into_iter()
            .zip(["out", "err"])
            .filter_map(|(fd, t)| fd.map(|fd| (fd, t)))
            .map(|(fd, t)| {
                let tx = tx.clone();
                let out = Arc::clone(out);
                let (from, to) = rewrite.clone();
                let pause = Arc::clone(&self.pause_pumps);
                std::thread::spawn(move || pump(fd, t, &from, &to, &tx, &out, &pause))
            })
            .collect()
    }

    /// Stops taking jobs and waits for running and queued ones to end.
    /// Clients that connect meanwhile find no socket and run locally.
    async fn drain(&self, listener: UnixListener) {
        drop(listener);
        let _ = std::fs::remove_file(&self.cfg.socket);
        let mut last = usize::MAX;
        loop {
            let n = self.inner.lock().expect("server lock").active.len();
            if n == 0 {
                eprintln!("slot: drained, exiting");
                return;
            }
            if n != last {
                eprintln!("slot: draining, {n} job(s) still running or queued");
                last = n;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn handle(self: Arc<Self>, stream: UnixStream) {
        let uid = stream.peer_cred().ok().map(|c| c.uid());
        let Ok(dup) = stream.as_fd().try_clone_to_owned() else {
            return;
        };
        let (rd, mut wr) = stream.into_split();
        let mut rd = TokioBufReader::new(rd);
        let mut line = String::new();
        if rd.read_line(&mut line).await.unwrap_or(0) == 0 {
            return;
        }
        let Ok(req) = serde_json::from_str::<Request>(&line) else {
            return;
        };
        if let Some(id) = req.cancel {
            let (code, msg) = self.cancel(id, uid).await;
            let mut out = Vec::new();
            for f in [Frame::Info { msg }, Frame::Exit { code }] {
                if let Ok(b) = serde_json::to_vec(&f) {
                    out.extend(b);
                    out.push(b'\n');
                }
            }
            let _ = wr.write_all(&out).await;
            let _ = wr.shutdown().await;
            return;
        }
        let a = self.attach(rd, wr, dup, uid, Vec::new());
        let stop = a.stop();
        let done = self
            .run_job(
                req,
                a.tx.clone(),
                a.gone_rx.clone(),
                Some(stop),
                Some(a.id),
                None,
            )
            .await;
        self.end_conn(Some(a), done.is_some()).await;
    }

    /// Sends a client its last frames unless a handover took the table;
    /// `None` then. Frames sent under the server lock are in the writer's
    /// queue before a handover flushes it.
    fn reply(
        &self,
        tx: &mpsc::UnboundedSender<Frame>,
        code: i32,
        msg: Option<String>,
    ) -> Option<()> {
        let g = self.inner.lock().expect("server lock");
        if g.handing_over {
            return None;
        }
        if let Some(msg) = msg {
            let _ = tx.send(Frame::Info { msg });
        }
        let _ = tx.send(Frame::Exit { code });
        Some(())
    }

    /// The job's environment before the toolchain adds to it.
    fn base_env(req: &Request) -> Vec<String> {
        if req.workspace.is_none() {
            return req.env.clone();
        }
        // Remote clients send only allowlisted variables; PATH, HOME and
        // the toolchain installs are the server's own.
        std::env::vars()
            .filter(|(k, _)| k != handover::ENV)
            .map(|(k, v)| format!("{k}={v}"))
            .chain(req.env.iter().cloned())
            .collect()
    }

    /// `slot cancel <id>` from a client with user id `uid`: stops a queued
    /// or running job the same way as its client going away (a running job's
    /// process group gets SIGTERM, then SIGKILL after 5 s). Waits up to 10 s
    /// for the job to end.
    async fn cancel(&self, id: i64, uid: Option<u32>) -> (i32, String) {
        let stop = {
            let g = self.inner.lock().expect("server lock");
            let Some(l) = g.active.get(&id) else {
                return (1, format!("slot: job {id} is not queued or running"));
            };
            match &l.stop {
                Some(s) if uid.is_some() && s.uid == uid => s.clone(),
                _ => return (1, format!("slot: job {id} belongs to another user")),
            }
        };
        let _ = stop.frames.send(Frame::Info {
            msg: format!("slot: job {id} cancelled by `slot cancel`"),
        });
        stop.gone.send_replace(true);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if !self
                .inner
                .lock()
                .expect("server lock")
                .active
                .contains_key(&id)
            {
                return (0, format!("slot: job {id} cancelled"));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        (1, format!("slot: job {id} still stopping after 10s"))
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

    /// Queues, runs and supervises one job. `resume` keeps the id and queue
    /// time of a job a handover passed on. `None` when a handover took the
    /// job: the client is answered by the next process.
    async fn run_job(
        self: &Arc<Self>,
        req: Request,
        tx: mpsc::UnboundedSender<Frame>,
        gone: watch::Receiver<bool>,
        stop: Option<Stop>,
        conn: Option<u64>,
        resume: Option<(i64, i64)>,
    ) -> Option<()> {
        if req.argv.is_empty() {
            return self.reply(&tx, 2, Some("slot: empty command".into()));
        }
        let (cwd, from, to) = match self.resolve(&req) {
            Ok(v) => v,
            Err(e) => return self.reply(&tx, 2, Some(format!("slot: {e}"))),
        };
        // A mistyped subcommand (`slot cancle 12`) would otherwise wait in
        // the queue for slots and fail only when it starts.
        let base = Self::base_env(&req);
        if !req.argv[0].contains('/')
            && let Err(e) = look_path(&req.argv[0], &base)
        {
            return self.reply(&tx, 127, Some(format!("slot: {e}; not queued")));
        }
        let tc = toolchain::for_argv(&req.argv, self.cfg.go_tokens);
        let limit = self.pool.limit();
        let tokens = job_tokens(tc.tokens(), req.tokens, self.pool.size(), limit);
        if let Some(n) = req.tokens.filter(|&n| n as usize > limit) {
            let _ = tx.send(Frame::Info {
                msg: format!("slot: --tokens {n} capped at {limit}, the slots in circulation"),
            });
        }
        let target_dir = (tc.name() == "rust" && req.workspace.is_none())
            .then(|| crate::evict::target_dir(&req.argv, &req.env, &cwd))
            .flatten();
        let out = Arc::new(Ring::new(OUTPUT_KEEP));
        let id = {
            let mut g = self.inner.lock().expect("server lock");
            let (id, enqueued_ms) = if let Some(r) = resume {
                r
            } else {
                g.next_id += 1;
                (g.next_id, now_ms())
            };
            let expect_rss = g.memory.expected(&req.repo, &req.argv, tc.name());
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
                enqueued_ms,
                started_ms: 0,
                ended_ms: 0,
                exit: 0,
                cpu_s: 0.0,
                compile_server_cpu_s: 0.0,
                peak_rss: 0,
                wait_reason: String::new(),
                target_dir: target_dir
                    .as_ref()
                    .map(|t| t.to_string_lossy().into_owned())
                    .unwrap_or_default(),
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
                    expect_rss,
                    stop,
                    req: Some(req.clone()),
                    waiter: None,
                    pumps: Vec::new(),
                    rewrite: (from.clone(), to.clone()),
                    exited: None,
                    conn,
                },
            );
            id
        };
        self.publish(id);
        if let Some(ws) = &req.workspace {
            self.prepare_cache(&ws.id, &req.repo, &tx).await;
        }

        let ahead = self.pool.waiting();
        let free = self.pool.free();
        if ahead > 0 || free < tokens {
            let _ = tx.send(Frame::Info {
                msg: format!(
                    "slot: job {id} queued for {tokens} slot(s): {free} free, {ahead} request(s) ahead"
                ),
            });
        }
        let mut disks = vec![cwd.clone()];
        if let Some(t) = toolchain::get(&req.env, "CARGO_TARGET_DIR").filter(|t| !t.is_empty()) {
            disks.push(cwd.join(t));
        }
        let cancelled = End {
            state: JobState::Cancelled,
            code: -1,
            cpu: None,
            client_code: 130,
            msg: None,
            note_wait: false,
        };
        if let Some(t) = &target_dir
            && !self.target_free(t, &tx, gone.clone()).await
        {
            return self.finish(id, &tx, cancelled).ok();
        }
        let held = loop {
            if !self.admission_open(gone.clone()).await {
                return self.finish(id, &tx, cancelled).ok();
            }
            if !self.resources_ready(id, &disks, &tx, gone.clone()).await {
                return self.finish(id, &tx, cancelled).ok();
            }
            let mut waiter = self.pool.acquire(tokens);
            if let Some(l) = self.inner.lock().expect("server lock").active.get_mut(&id) {
                l.waiter = Some(waiter.handle());
            }
            tokio::select! {
                () = waiter.ready() => {}
                () = client_gone(gone.clone()) => {
                    if self.pool.cancel(&waiter) {
                        self.pool.release(waiter.take_held());
                    }
                    return self.finish(id, &tx, cancelled).ok();
                }
            }
            // Memory may have gone while the job waited for slots: give
            // them back and wait for memory first.
            if self.short_of(id, &disks).is_some() {
                self.pool.release(waiter.take_held());
                continue;
            }
            // Claim the slots under the server lock: an upgrade requested
            // meanwhile (hold or handover) sends them back and the job waits
            // with the others; a handover sees the claim and waits for the
            // process.
            let claimed = {
                let mut g = self.inner.lock().expect("server lock");
                if *self.upgrading.borrow() || g.handing_over {
                    None
                } else {
                    let n = waiter.take_held();
                    g.active.get_mut(&id).map(|l| {
                        l.held = n;
                        l.waiter = None;
                        n
                    })
                }
            };
            match claimed {
                Some(n) => break n,
                None => {
                    self.pool.release(waiter.take_held());
                }
            }
        };

        let ctx = Context {
            jobserver_auth: self.pool.auth(),
            held,
            free_tokens: self.pool.free(),
            test_threads: self.cfg.test_threads,
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
                "--slice",
                JOB_SLICE,
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
        let failed = |msg: String| End {
            state: JobState::Failed,
            code: 127,
            cpu: None,
            client_code: 127,
            msg: Some(msg),
            note_wait: false,
        };
        let program = match look_path(&argv[0], &env) {
            Ok(p) => p,
            Err(e) => return self.finish(id, &tx, failed(format!("slot: {e}"))).ok(),
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
                return self
                    .finish(id, &tx, failed(format!("slot: {}: {e}", argv[0])))
                    .ok();
            }
        };
        let pid = child.id() as i32;
        let pipes = [
            child.stdout.take().map(OwnedFd::from),
            child.stderr.take().map(OwnedFd::from),
        ];
        // The process is reaped by pid (`reap`), here or after a handover.
        drop(child);
        let target_dir = req.workspace.as_ref().map_or_else(String::new, |w| {
            self.cfg
                .cache_root
                .join("target")
                .join(&w.id)
                .to_string_lossy()
                .into_owned()
        });
        let pumps = self.start_pumps(pipes, &tx, &out, &(from, to));
        {
            let mut g = self.inner.lock().expect("server lock");
            if let Some(l) = g.active.get_mut(&id) {
                l.pid = Some(pid);
                l.job.started_ms = now_ms();
                l.job.state = JobState::Running;
                l.tracker.sample(&Snapshot::scan(), pid, &[], 0.0);
                l.req = None;
                l.pumps = pumps;
            }
            let _ = tx.send(Frame::Start { id, target_dir });
        }
        self.publish(id);
        let done = self.supervise(id, pid, &tx, gone).await;
        if done.is_some()
            && let Some(ws) = &req.workspace
        {
            let cache = cache::Cache::new(&self.cfg.cache_root, &self.cfg.ws_root);
            let ws = ws.id.clone();
            tokio::task::spawn_blocking(move || cache.record_size(&ws));
        }
        done
    }

    /// Waits for job `id`'s process `pid` (or kills its group when the
    /// client goes away), lets its output drain and finishes it. `None`
    /// when a handover took the job first.
    async fn supervise(
        self: &Arc<Self>,
        id: i64,
        pid: i32,
        tx: &mpsc::UnboundedSender<Frame>,
        gone: watch::Receiver<bool>,
    ) -> Option<()> {
        let s = Arc::clone(self);
        let wait = tokio::task::spawn_blocking(move || s.reap(id, pid));
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
        let exited = result.ok().flatten()?;
        // A grandchild that keeps a pipe open would block forever: give the
        // readers 2 s after the job's main process exits.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let drained = {
                let g = self.inner.lock().expect("server lock");
                g.active
                    .get(&id)
                    .is_none_or(|l| l.pumps.iter().all(std::thread::JoinHandle::is_finished))
            };
            if drained || Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let end = if cancelled {
            End {
                state: JobState::Cancelled,
                code: -1,
                cpu: exited.cpu,
                client_code: 130,
                msg: None,
                note_wait: false,
            }
        } else {
            End {
                state: if exited.code == 0 {
                    JobState::Ok
                } else {
                    JobState::Failed
                },
                code: exited.code,
                cpu: exited.cpu,
                client_code: exited.code,
                msg: None,
                note_wait: true,
            }
        };
        self.finish(id, tx, end).ok()
    }

    /// Waits for job `id`'s process to exit, reads its CPU time while it is
    /// still a zombie, then reaps it, unless a handover took the table:
    /// then the zombie stays for the next process. A job a handover found
    /// reaped already gets its recorded exit.
    fn reap(&self, id: i64, pid: i32) -> Option<handover::Exited> {
        if let Some(e) = self
            .inner
            .lock()
            .expect("server lock")
            .active
            .get(&id)
            .and_then(|l| l.exited)
        {
            return Some(e);
        }
        let p = Pid::from_raw(pid);
        while let Err(nix::errno::Errno::EINTR) =
            waitid(Id::Pid(p), WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT)
        {}
        let cpu = procstat::total_cpu_seconds(pid);
        let mut g = self.inner.lock().expect("server lock");
        if g.handing_over {
            return None;
        }
        let code = loop {
            match waitpid(p, None) {
                Ok(WaitStatus::Exited(_, c)) => break c,
                Ok(WaitStatus::Signaled(_, s, _)) => break 128 + s as i32,
                Err(nix::errno::Errno::EINTR) => {}
                _ => break 127,
            }
        };
        let e = handover::Exited { code, cpu };
        if let Some(l) = g.active.get_mut(&id) {
            l.exited = Some(e);
        }
        Some(e)
    }

    /// Workspace ids with a queued or running job.
    fn busy_workspaces(&self) -> HashSet<String> {
        let g = self.inner.lock().expect("server lock");
        g.active
            .values()
            .filter(|l| l.job.remote)
            .filter_map(|l| {
                Path::new(&l.job.cwd)
                    .strip_prefix(&self.cfg.ws_root)
                    .ok()
                    .and_then(|r| r.components().next())
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
            })
            .collect()
    }

    /// Before a remote job of workspace `ws` (repository `repo`) queues:
    /// records the use, evicts least recently used target directories over
    /// the budget or the disk floor, and seeds a new workspace's target
    /// directory by reflink. Says what it did on the job's stderr and in the
    /// journal.
    async fn prepare_cache(&self, ws: &str, repo: &str, tx: &mpsc::UnboundedSender<Frame>) {
        let cache = cache::Cache::new(&self.cfg.cache_root, &self.cfg.ws_root);
        let busy = self.busy_workspaces();
        let budget = self.cfg.cache_budget;
        let floor = self.cfg.admission.min_free_disk;
        let (ws, repo) = (ws.to_string(), repo.to_string());
        let Ok((gone, seeded)) = tokio::task::spawn_blocking(move || {
            cache.touch(&ws, &repo, now_ms());
            let gone = cache.evict(budget, floor, &busy);
            let seeded = cache.seed(&ws, &repo, &busy);
            (gone, seeded)
        })
        .await
        else {
            return;
        };
        for (id, bytes) in gone {
            let msg = format!(
                "slot: deleted the target directory of workspace {id} ({}), least recently used",
                admission::gb(bytes)
            );
            eprintln!("{msg}");
            let _ = tx.send(Frame::Info { msg });
        }
        match seeded {
            Ok((src, took)) => {
                let msg = format!(
                    "slot: target directory seeded from workspace {src} by reflink in {:.1}s",
                    took.as_secs_f64()
                );
                eprintln!("{msg}");
                let _ = tx.send(Frame::Info { msg });
            }
            Err(e) if e.contains("reflink") => eprintln!("slot: target directory not seeded: {e}"),
            Err(_) => {}
        }
    }

    /// Why job `id` may not start now: its filesystems (`disks`) are short
    /// of space, or memory is short and another job runs. `(kind, text)`.
    fn short_of(&self, id: i64, disks: &[PathBuf]) -> Option<(&'static str, String)> {
        let limits = self.cfg.admission;
        let free: Vec<(String, u64)> = disks
            .iter()
            .filter_map(|p| admission::disk_free(p).map(|f| (p.display().to_string(), f)))
            .collect();
        if let Some(r) = admission::disk_wait(&free, limits) {
            return Some(("disk", r));
        }
        if limits.min_free_memory == 0 {
            return None;
        }
        let available = admission::mem_available()?;
        let now = now_ms();
        let (expected, reserved, others) = {
            let g = self.inner.lock().expect("server lock");
            let expected = g.active.get(&id).map_or(0, |l| l.expect_rss);
            let mut reserved = 0;
            let mut others = false;
            for (other, l) in &g.active {
                if *other == id || l.pid.is_none() {
                    continue;
                }
                others = true;
                if now - l.job.started_ms < STARTING_MS {
                    reserved += l.expect_rss.saturating_sub(l.rss);
                }
            }
            (expected, reserved, others)
        };
        admission::memory_wait(available, reserved, expected, limits, others).map(|r| ("memory", r))
    }

    /// Waits until job `id` may start as far as disk and memory go, telling
    /// the client why it waits; false when the client went away first.
    async fn resources_ready(
        &self,
        id: i64,
        disks: &[PathBuf],
        tx: &mpsc::UnboundedSender<Frame>,
        gone: watch::Receiver<bool>,
    ) -> bool {
        let mut told = "";
        loop {
            let short = self.short_of(id, disks);
            let reason = short.as_ref().map(|(_, r)| r.clone()).unwrap_or_default();
            let changed = {
                let mut g = self.inner.lock().expect("server lock");
                g.active.get_mut(&id).is_some_and(|l| {
                    let c = l.job.wait_reason != reason;
                    l.job.wait_reason.clone_from(&reason);
                    c
                })
            };
            if changed {
                self.publish(id);
            }
            let Some((kind, text)) = short else {
                return true;
            };
            if kind != told {
                let _ = tx.send(Frame::Info {
                    msg: format!("slot: job {id} waiting: {text}"),
                });
                eprintln!("slot: job {id} waiting: {text}");
                told = kind;
            }
            tokio::select! {
                () = tokio::time::sleep(RESOURCE_POLL) => {}
                () = client_gone(gone.clone()) => return false,
            }
        }
    }

    /// Finishes job `id`: logs it, returns its slots and answers its client,
    /// all under the server lock, so a handover sees the job either before
    /// or after, never half. `Err` when a handover took the table.
    fn finish(
        &self,
        id: i64,
        tx: &mpsc::UnboundedSender<Frame>,
        end: End,
    ) -> Result<(), HandedOver> {
        let (job, idle) = {
            let mut g = self.inner.lock().expect("server lock");
            if g.handing_over {
                return Err(HandedOver);
            }
            let Some(mut live) = g.active.remove(&id) else {
                return Ok(());
            };
            if let Some(w) = live.waiter.take() {
                self.pool.release(w.take());
            }
            for h in std::mem::take(&mut live.pumps) {
                if h.is_finished() {
                    let _ = h.join();
                }
            }
            live.job.state = end.state;
            live.job.exit = end.code;
            live.job.ended_ms = now_ms();
            live.job.cpu_s = live.job.compile_server_cpu_s + end.cpu.unwrap_or(0.0);
            let job = live.job.clone();
            g.metrics.record(&job);
            g.memory.record(&job);
            record_target(&mut g.targets, &job);
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
            self.pool.release(live.held);
            let wait = job.started_ms - job.enqueued_ms;
            let msg = end.msg.or_else(|| {
                (end.note_wait && job.started_ms > 0 && wait >= 2000).then(|| {
                    format!(
                        "slot: job {} waited {}s for a build slot, ran {}s",
                        job.id,
                        wait / 1000,
                        (job.ended_ms - job.started_ms) / 1000
                    )
                })
            });
            if let Some(msg) = msg {
                let _ = tx.send(Frame::Info { msg });
            }
            let _ = tx.send(Frame::Exit {
                code: end.client_code,
            });
            (job, g.active.is_empty())
        };
        self.send(&Event::Job { job });
        if idle {
            let n = self.pool.reconcile();
            if n != 0 {
                eprintln!("slot: jobserver reconciled by {n:+} tokens");
            }
        }
        Ok(())
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
            if matches!(e, Event::Tick { .. }) {
                let mut h = self.history.lock().expect("history lock");
                h.push_back(s.clone());
                while h.len() > HISTORY_TICKS {
                    h.pop_front();
                }
            }
            let _ = self.events.send(s);
        }
    }

    /// The last ticks as one JSON array, oldest first (`GET /api/history`).
    pub fn history_json(&self) -> String {
        let h = self.history.lock().expect("history lock");
        let mut out = String::with_capacity(h.iter().map(String::len).sum::<usize>() + h.len() + 2);
        out.push('[');
        for (i, t) in h.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(t);
        }
        out.push(']');
        out
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
            let mut new_ids = Vec::new();
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
                let mut managed: HashSet<i32> = HashSet::new();
                for l in g.active.values() {
                    if let Some(pid) = l.pid {
                        managed.extend(snap.tree(pid).iter().map(|p| p.pid));
                    }
                }
                for procs in extra.values() {
                    managed.extend(procs.iter().map(|p| p.pid));
                }
                let sample = g.census.sample(&snap, &managed, dt);
                let used = self.pool.limit().saturating_sub(self.pool.free());
                let waiting = self.pool.waiting();
                g.window.add(now_ms(), &sample, used, waiting);
                if g.window.len() >= CENSUS_WINDOW {
                    let line = g.window.flush();
                    if let Ok(mut b) = serde_json::to_vec(&line) {
                        b.push(b'\n');
                        let _ = g.census_log.write_all(&b);
                    }
                }
                for id in sample.sessions.keys() {
                    if !id.is_empty()
                        && !g.titles.contains_key(id)
                        && g.title_pending.insert(id.clone())
                    {
                        new_ids.push(id.clone());
                    }
                }
                let mut census = census::view(&sample, &g.titles);
                census.recommended_tokens = g.controller.step(&census) as u32;
                census.unmanaged_smoothed = g.controller.unmanaged();
                g.window.recommend(census.recommended_tokens);
                g.last_census = census.clone();
                Event::Tick {
                    t: now_ms(),
                    tokens: self.cfg.tokens as u32,
                    limit: self.pool.limit() as u32,
                    headroom_percent: u32::from(self.headroom_percent()),
                    call_apps: self.call_apps(),
                    free: self.pool.free() as u32,
                    waiting: self.pool.waiting() as u32,
                    sys_cpu: g.sys_cpu,
                    job_cores: total,
                    jobs,
                    census,
                    upgrade: self
                        .upgrade
                        .lock()
                        .expect("upgrade lock")
                        .describe(Instant::now()),
                }
            };
            self.send(&event);
            for id in new_ids {
                let s = Arc::clone(&self);
                tokio::task::spawn_blocking(move || {
                    let title = census::session_title(&id);
                    let mut g = s.inner.lock().expect("server lock");
                    g.title_pending.remove(&id);
                    g.titles.insert(id, title);
                });
            }
        }
    }

    /// The `GET /metrics` exposition text.
    pub fn metrics_text(&self) -> String {
        let g = self.inner.lock().expect("server lock");
        let mut gauges = metrics::Gauges {
            tokens: self.cfg.tokens,
            free: self.pool.free(),
            waiting: self.pool.waiting(),
            restored: self.restored.load(Ordering::Relaxed),
            job_cores: g.job_cores,
            sys_cpu: g.sys_cpu,
            census: g.last_census.clone(),
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

    fn headroom_percent(&self) -> u8 {
        self.headroom.lock().expect("headroom lock").percent
    }

    /// The `GET /api/limits` answer.
    pub fn limits(&self) -> limits::View {
        let h = self.headroom.lock().expect("headroom lock").clone();
        let now = Instant::now();
        limits::View {
            headroom_percent: h.percent,
            cpu_quota_percent: h.quota,
            effective_tokens: self.pool.limit(),
            tokens: self.cfg.tokens,
            systemd: own_unit().is_some(),
            note: h.note,
            call_headroom_percent: h.call_percent,
            call: limits::CallView {
                active: h.call.active(),
                apps: h.call.apps().to_vec(),
                base_percent: if h.call.active() {
                    h.call.base()
                } else {
                    h.percent
                },
                manual: h.call.is_manual(),
                ends_in_s: h.call.ends_in(now, self.cfg.call_off_after),
                note: h.call_note,
            },
        }
    }

    /// Applications in a call while call mode is on; empty otherwise.
    fn call_apps(&self) -> Vec<String> {
        let h = self.headroom.lock().expect("headroom lock");
        if h.call.active() {
            h.call.apps().to_vec()
        } else {
            Vec::new()
        }
    }

    /// Sets the headroom by hand (`slot headroom`, the UI slider): saved, and
    /// during a call it stays when the call ends.
    pub async fn set_headroom(self: Arc<Self>, percent: u8) -> limits::View {
        self.headroom.lock().expect("headroom lock").call.manual();
        self.apply_headroom(percent, true).await
    }

    /// Applies a `PUT /api/limits` body: the call headroom first, then the
    /// headroom.
    pub async fn update_limits(self: Arc<Self>, req: limits::Request) -> limits::View {
        if let Some(c) = req.call_headroom_percent {
            let c = c.min(limits::MAX_HEADROOM);
            let action = {
                let mut h = self.headroom.lock().expect("headroom lock");
                h.call_percent = c;
                h.call_saved = Some(c);
                h.call.retarget(c)
            };
            self.save_limits();
            match action {
                call::Action::Raise(p) => {
                    self.apply_headroom(p, false).await;
                }
                call::Action::Restore(p) => {
                    self.apply_headroom(p, true).await;
                }
                call::Action::Keep => {}
            }
        }
        match req.headroom_percent {
            Some(p) => Arc::clone(&self).set_headroom(p).await,
            None => self.limits(),
        }
    }

    fn save_limits(&self) {
        let saved = {
            let h = self.headroom.lock().expect("headroom lock");
            limits::Saved {
                headroom_percent: h.saved_percent,
                call_headroom_percent: h.call_saved,
            }
        };
        if let Err(e) = limits::save(&self.cfg.state_dir, saved) {
            eprintln!("slot: save {}: {e}", limits::FILE);
        }
    }

    /// Every 15 s while requests wait, writes back jobserver tokens that a
    /// process took and never returned (a compiler killed by a cancel or a
    /// `timeout`). The idle-time reconcile cannot do it on a pool that is
    /// never idle. It counts only while no process but the daemon has the
    /// FIFO open, so no token can be in a live process, and acts on the
    /// smaller of two shortfalls 15 s apart, so a token the reader thread
    /// is booking at that moment is not counted as lost.
    async fn token_watch(self: Arc<Self>) {
        let mut tick = tokio::time::interval(TOKEN_WATCH);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last: Option<usize> = None;
        loop {
            tick.tick().await;
            if self.pool.waiting() == 0 || self.pool.free() > 0 {
                last = None;
                continue;
            }
            let path = self.pool.path().to_path_buf();
            let Ok(others) =
                tokio::task::spawn_blocking(move || jobserver::holders(&path, std::process::id()))
                    .await
            else {
                continue;
            };
            if !others.is_empty() {
                last = None;
                continue;
            }
            let granted: usize = {
                let g = self.inner.lock().expect("server lock");
                g.active.values().map(|l| l.held).sum()
            };
            let missing = self.pool.missing(granted);
            let n = last.map_or(0, |prev| prev.min(missing));
            last = (missing > 0).then_some(missing);
            if n == 0 {
                continue;
            }
            self.pool.restore(n);
            self.restored.fetch_add(n as u64, Ordering::Relaxed);
            last = None;
            eprintln!(
                "slot: {n} jobserver token(s) were lost (no process holds the FIFO, \
                 {granted} granted to jobs, {} waiting): wrote them back",
                self.pool.waiting()
            );
            let line = serde_json::json!({
                "t": now_ms(),
                "event": "tokens_restored",
                "tokens": n,
                "granted": granted,
            });
            let mut g = self.inner.lock().expect("server lock");
            if let Ok(mut b) = serde_json::to_vec(&line) {
                b.push(b'\n');
                let _ = g.census_log.write_all(&b);
            }
        }
    }

    /// Every 5 s, gives back the jobserver tokens an sccache server lost
    /// (see [`crate::sccache`]). Each refill goes to the journal and to the
    /// census log.
    async fn sccache_watch(self: Arc<Self>) {
        let dog = Arc::new(Mutex::new(crate::sccache::Watchdog::new(
            self.cores as usize,
        )));
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let d = Arc::clone(&dog);
            let Ok(pass) = tokio::task::spawn_blocking(move || {
                let snap = Snapshot::scan();
                d.lock().expect("watchdog lock").pass(&snap)
            })
            .await
            else {
                continue;
            };
            let now = now_ms();
            {
                let mut w = self.sccache.lock().expect("sccache lock");
                w.servers = pass.servers as u32;
                w.short = pass.short as u32;
                for r in &pass.refills {
                    w.refills += 1;
                    w.tokens += r.tokens as u32;
                    w.last_refill_ms = now;
                }
            }
            for r in &pass.refills {
                eprintln!(
                    "slot: sccache server {} (port {}) had no free token for 60 s with {} compile(s) \
                     running and {} client(s): wrote {} token(s) back",
                    r.pid, r.port, r.running, r.clients, r.tokens
                );
                let line = serde_json::json!({
                    "t": now,
                    "event": "sccache_refill",
                    "pid": r.pid,
                    "port": r.port,
                    "tokens": r.tokens,
                    "running": r.running,
                    "clients": r.clients,
                });
                let mut g = self.inner.lock().expect("server lock");
                if let Ok(mut b) = serde_json::to_vec(&line) {
                    b.push(b'\n');
                    let _ = g.census_log.write_all(&b);
                }
            }
        }
    }

    /// Waits while eviction deletes target directory `t`; false when the
    /// client went away first.
    async fn target_free(
        &self,
        t: &Path,
        tx: &mpsc::UnboundedSender<Frame>,
        gone: watch::Receiver<bool>,
    ) -> bool {
        let mut told = false;
        while self.evicting.lock().expect("evicting lock").contains(t) {
            if !told {
                let _ = tx.send(Frame::Info {
                    msg: format!(
                        "slot: waiting: {} is being deleted to free disk space",
                        t.display()
                    ),
                });
                told = true;
            }
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(500)) => {}
                () = client_gone(gone.clone()) => return false,
            }
        }
        true
    }

    /// Every 60 s, when the filesystem of a target directory jobs built into
    /// has less free space than the floor, deletes the least recently built
    /// ones until it has the floor plus the margin; in a dry run it only
    /// says which. See [`crate::evict`].
    async fn evict_watch(self: Arc<Self>) {
        let s = self.cfg.evict;
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_said = Vec::new();
        loop {
            tick.tick().await;
            let (seen, busy_repos, busy_dirs) = {
                let g = self.inner.lock().expect("server lock");
                let repos: HashSet<String> =
                    g.active.values().map(|l| l.job.repo.clone()).collect();
                let dirs: HashSet<PathBuf> = g
                    .active
                    .values()
                    .filter(|l| !l.job.target_dir.is_empty())
                    .map(|l| PathBuf::from(&l.job.target_dir))
                    .collect();
                (g.targets.clone(), repos, dirs)
            };
            let known = seen.len() as u32;
            let Ok((free, planned)) = tokio::task::spawn_blocking(move || {
                evict_pass(&seen, &busy_repos, &busy_dirs, now_ms(), s)
            })
            .await
            else {
                continue;
            };
            let mut done = Vec::new();
            for (c, bytes) in planned {
                let action = if s.dry_run {
                    "would delete".to_string()
                } else {
                    self.evict_one(&c).await
                };
                done.push(crate::evict::Eviction {
                    path: c.path.to_string_lossy().into_owned(),
                    bytes,
                    last_ms: c.last_ms,
                    action,
                });
            }
            let said: Vec<(String, String)> = done
                .iter()
                .map(|e| (e.path.clone(), e.action.clone()))
                .collect();
            if !done.is_empty() && (said != last_said || !s.dry_run) {
                for e in &done {
                    eprintln!(
                        "slot: disk: {} free, under {}: {} {} ({}, last built {} ago)",
                        admission::gb(free),
                        admission::gb(s.floor),
                        e.action,
                        e.path,
                        admission::gb(e.bytes),
                        crate::status::secs(now_ms() - e.last_ms)
                    );
                }
            }
            last_said = said;
            let mut r = self.evict.lock().expect("evict lock");
            r.known = known;
            r.free = free;
            r.checked_ms = now_ms();
            if !done.is_empty() || free >= s.floor {
                r.deleted_bytes += done
                    .iter()
                    .filter(|e| e.action == "deleted" || e.action == "discard-cache")
                    .map(|e| e.bytes)
                    .sum::<u64>();
                r.last = done;
            }
        }
    }

    /// Deletes one target directory unless a job of its repository or
    /// directory started meanwhile; returns what happened.
    async fn evict_one(&self, c: &crate::evict::Candidate) -> String {
        {
            let g = self.inner.lock().expect("server lock");
            let busy = g
                .active
                .values()
                .any(|l| l.job.repo == c.repo || Path::new(&l.job.target_dir) == c.path.as_path());
            if busy {
                return "kept: a job started".into();
            }
            self.evicting
                .lock()
                .expect("evicting lock")
                .insert(c.path.clone());
        }
        let path = c.path.clone();
        let r = tokio::task::spawn_blocking(move || crate::evict::remove(&path))
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        self.evicting.lock().expect("evicting lock").remove(&c.path);
        match r {
            Ok(a) => a.to_string(),
            Err(e) => format!("failed: {e}"),
        }
    }

    /// Watches for calls and moves the headroom: up when one starts, back
    /// [`Config::call_off_after`] after the last capture.
    async fn call_watch(self: Arc<Self>) {
        let have_pw = tokio::task::spawn_blocking(|| {
            Command::new("pw-dump")
                .arg("--version")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
        .await
        .unwrap_or(false);
        if !have_pw {
            self.headroom.lock().expect("headroom lock").call_note =
                "pw-dump not found: only camera use is seen".into();
        }
        let mut tick = tokio::time::interval(CALL_PROBE);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let percent = self.headroom.lock().expect("headroom lock").call_percent;
            if percent == 0 {
                continue;
            }
            let ignore = self.cfg.call_ignore.clone();
            let apps = tokio::task::spawn_blocking(move || call::probe(&ignore))
                .await
                .unwrap_or_default();
            let (action, current, seen) = {
                let mut h = self.headroom.lock().expect("headroom lock");
                let current = h.percent;
                let action = h.call.observe(
                    Instant::now(),
                    apps,
                    current,
                    percent,
                    self.cfg.call_off_after,
                );
                (action, current, h.call.apps().join(", "))
            };
            match action {
                call::Action::Raise(p) => {
                    eprintln!("slot: call started ({seen}): headroom {current}% -> {p}%");
                    self.apply_headroom(p, false).await;
                }
                call::Action::Restore(p) => {
                    eprintln!("slot: call ended: headroom {current}% -> {p}%");
                    self.apply_headroom(p, true).await;
                }
                call::Action::Keep => {}
            }
        }
    }

    /// Sets the headroom: lowers the circulating tokens, saves it when
    /// `persist`, and sets `CPUQuota` on the daemon's unit when there is one.
    async fn apply_headroom(self: &Arc<Self>, percent: u8, persist: bool) -> limits::View {
        let percent = percent.min(limits::MAX_HEADROOM);
        self.pool
            .set_limit(limits::effective_tokens(self.cfg.tokens, percent));
        if persist {
            self.headroom.lock().expect("headroom lock").saved_percent = percent;
            self.save_limits();
        }
        let quota = limits::cpu_quota_percent(self.cores, percent);
        let slice = self.cfg.memory_max.is_some();
        let applied = if self.cfg.cpu_quota {
            tokio::task::spawn_blocking(move || {
                let unit = own_unit();
                limits::apply_quota(unit.as_deref(), quota)?;
                // Jobs under --memory-max run in scopes in JOB_SLICE, outside
                // the daemon's unit: the same quota covers them there.
                if slice && unit.is_some() {
                    limits::apply_quota(Some(JOB_SLICE), quota)?;
                }
                Ok(())
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()))
        } else {
            Err("cpu quota disabled (--cpu-quota false)".into())
        };
        {
            let mut h = self.headroom.lock().expect("headroom lock");
            h.percent = percent;
            match applied {
                Ok(()) => {
                    h.quota = quota;
                    h.note = String::new();
                }
                Err(e) => {
                    h.quota = None;
                    h.note = e;
                }
            }
        }
        self.limits()
    }

    /// The `GET /api/state` snapshot.
    pub fn state(&self) -> State {
        let g = self.inner.lock().expect("server lock");
        State {
            host: self.host.clone(),
            cores: self.cores,
            tokens: self.cfg.tokens as u32,
            limit: self.pool.limit() as u32,
            headroom_percent: u32::from(self.headroom_percent()),
            call_apps: self.call_apps(),
            cpu_quota_percent: self.headroom.lock().expect("headroom lock").quota,
            free: self.pool.free() as u32,
            active: g.active.values().map(|l| l.job.clone()).collect(),
            done: g.done.iter().map(|d| d.job.clone()).collect(),
            now_ms: now_ms(),
            toolchains: toolchain::names().into_iter().map(String::from).collect(),
            sccache: self.sccache.lock().expect("sccache lock").clone(),
            evict: self.evict.lock().expect("evict lock").clone(),
            upgrade: self
                .upgrade
                .lock()
                .expect("upgrade lock")
                .describe(Instant::now()),
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

/// How long a pump waits for output before it looks at the pause flag.
const PUMP_POLL_MS: u16 = 100;

/// Copies one output pipe of a job to its client and its ring until the end
/// of the stream, or until `pause` is set: then it returns the pipe, so a
/// handover can pass it on with no byte read and not sent.
fn pump(
    fd: OwnedFd,
    t: &'static str,
    from: &str,
    to: &str,
    tx: &mpsc::UnboundedSender<Frame>,
    out: &Ring,
    pause: &AtomicBool,
) -> PumpEnd {
    let mut p = File::from(fd);
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
        if pause.load(Ordering::SeqCst) {
            emit(rw.finish());
            return (t, Some(OwnedFd::from(p)));
        }
        let mut fds = [PollFd::new(p.as_fd(), PollFlags::POLLIN)];
        match poll(&mut fds, PUMP_POLL_MS) {
            Ok(0) | Err(nix::errno::Errno::EINTR) => continue,
            Ok(_) => {}
            Err(_) => break,
        }
        match p.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => emit(rw.push(&buf[..n])),
            Err(e) if matches!(e.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) => {}
            Err(_) => break,
        }
    }
    emit(rw.finish());
    (t, None)
}

/// Writes frames to one client. Bytes are queued in `pending` and written as
/// the socket takes them; a request on `ctl` gets every unsent byte, and the
/// writer then writes nothing more, for a handover to pass on. Ends after
/// the exit frame, or when every sender is gone, once `pending` is written.
async fn client_writer(
    mut wr: OwnedWriteHalf,
    mut rx: mpsc::UnboundedReceiver<Frame>,
    mut ctl: mpsc::UnboundedReceiver<oneshot::Sender<Vec<u8>>>,
    mut pending: Vec<u8>,
) {
    let mut rx_open = true;
    let mut ctl_open = true;
    let mut closing = false;
    loop {
        if (closing || !rx_open) && pending.is_empty() {
            let _ = wr.shutdown().await;
            return;
        }
        tokio::select! {
            biased;
            c = ctl.recv(), if ctl_open => match c {
                Some(reply) => {
                    let _ = reply.send(std::mem::take(&mut pending));
                    std::future::pending::<()>().await;
                }
                None => ctl_open = false,
            },
            f = rx.recv(), if rx_open && !closing => match f {
                Some(f) => {
                    if let Ok(b) = serde_json::to_vec(&f) {
                        pending.extend_from_slice(&b);
                        pending.push(b'\n');
                    }
                    closing = matches!(f, Frame::Exit { .. });
                }
                None => rx_open = false,
            },
            r = wr.writable(), if !pending.is_empty() => {
                if r.is_err() {
                    return;
                }
                match wr.try_write(&pending) {
                    Ok(n) => {
                        pending.drain(..n);
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                    Err(_) => return,
                }
            }
        }
    }
}

/// The binary installed at this process's path, which a reload executes.
fn installed_exe() -> PathBuf {
    std::fs::read_link("/proc/self/exe").map_or_else(
        |_| PathBuf::from("slot"),
        |p| {
            let s = p.to_string_lossy();
            PathBuf::from(s.strip_suffix(" (deleted)").unwrap_or(&s))
        },
    )
}

/// Why a handover did not happen.
enum Abort {
    /// Nothing changed for good; the bounded hold takes over.
    Retry(String),
    /// The job table is given up; the daemon can only exit.
    Fatal(anyhow::Error),
}

/// The job table a handover takes, with each job's connection id.
#[derive(Default)]
struct Table {
    running: Vec<(handover::Running, Option<u64>)>,
    queued: Vec<(handover::Queued, Option<u64>)>,
    pumps: Vec<Vec<std::thread::JoinHandle<PumpEnd>>>,
    next_id: i64,
}

/// How a job ends: what is logged and what the client is told.
struct End {
    state: JobState,
    code: i32,
    cpu: Option<f64>,
    client_code: i32,
    msg: Option<String>,
    /// Tell the client how long it waited for slots, when that was 2 s or more.
    note_wait: bool,
}

/// A registered client connection.
struct Attached {
    id: u64,
    uid: Option<u32>,
    tx: mpsc::UnboundedSender<Frame>,
    gone_tx: Arc<watch::Sender<bool>>,
    gone_rx: watch::Receiver<bool>,
    writer: tokio::task::JoinHandle<()>,
}

impl Attached {
    fn stop(&self) -> Stop {
        Stop {
            uid: self.uid,
            gone: Arc::clone(&self.gone_tx),
            frames: self.tx.clone(),
        }
    }
}

/// One eviction look, blocking: the lowest free space among the
/// filesystems of known target directories, and per filesystem under the
/// floor the candidates to delete with their sizes. Sizes are measured
/// oldest first and only until enough would be freed.
fn evict_pass(
    seen: &HashMap<PathBuf, (String, i64)>,
    busy_repos: &HashSet<String>,
    busy_dirs: &HashSet<PathBuf>,
    now: i64,
    s: crate::evict::Settings,
) -> (u64, Vec<(crate::evict::Candidate, u64)>) {
    use crate::evict;
    let free = seen
        .keys()
        .filter(|p| p.exists())
        .filter_map(|p| admission::disk_free(p))
        .min()
        .unwrap_or(0);
    let cands: Vec<evict::Candidate> =
        evict::candidates(seen, busy_repos, busy_dirs, now, s.min_idle)
            .into_iter()
            .filter(|c| evict::is_target_dir(&c.path))
            .collect();
    let mut out = Vec::new();
    for group in evict::by_device(cands).into_values() {
        let Some(fs_free) = group.first().and_then(|c| admission::disk_free(&c.path)) else {
            continue;
        };
        if fs_free >= s.floor {
            continue;
        }
        let goal = s.floor.saturating_add(s.margin);
        let mut sized = Vec::new();
        let mut have = fs_free;
        for c in group {
            if have >= goal {
                break;
            }
            let b = evict::dir_size(&c.path);
            have = have.saturating_add(b);
            sized.push((c, b));
        }
        out.extend(evict::plan(fs_free, s, &sized).into_iter().cloned());
    }
    (free, out)
}

/// Notes that `job` built into its target directory.
fn record_target(targets: &mut HashMap<PathBuf, (String, i64)>, job: &Job) {
    if job.target_dir.is_empty() || job.started_ms == 0 {
        return;
    }
    let when = job.ended_ms.max(job.started_ms);
    let e = targets
        .entry(PathBuf::from(&job.target_dir))
        .or_insert_with(|| (job.repo.clone(), when));
    if when >= e.1 {
        *e = (job.repo.clone(), when);
    }
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
