//! The client side of `slot <command…>`.
//!
//! Local mode talks to the daemon on this machine through its unix socket.
//! Remote mode (`SLOT_HOST` set) rsyncs the repository to the server and
//! talks to the daemon through `ssh $SLOT_HOST slot proxy`. When no daemon
//! answers, the call waits for one and then exits without running the
//! command, unless `SLOT_LOCAL_FALLBACK` asks it to run here.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};

use std::fmt::Write as _;

use anyhow::{Context as _, Result, anyhow};
use sha2::{Digest, Sha256};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};

use crate::artifacts;
use crate::never_sync;
use crate::paths;
use crate::proto::{Frame, Request, Workspace};
use crate::repo_config::{self, RepoConfig};
use crate::same_path;
use crate::sync;
use crate::toolchain;
use crate::types::Checkout;

/// How long a queued job waits for an upgrading daemon to come back.
pub const UPGRADE_WAIT: Duration = Duration::from_secs(600);

/// Interval between connection attempts while the daemon upgrades.
const UPGRADE_POLL: Duration = Duration::from_millis(500);

/// The signal that ended this client (SIGINT, SIGTERM or SIGHUP), 0 while none.
static SIGNALLED: AtomicI32 = AtomicI32::new(0);

/// Closes the current connection to the daemon, so it cancels the job.
type Closer = Box<dyn FnMut() + Send>;
static CLOSER: Mutex<Option<Closer>> = Mutex::new(None);

/// The signal that ended this client, if one did.
fn signalled() -> Option<i32> {
    match SIGNALLED.load(Ordering::SeqCst) {
        0 => None,
        s => Some(s),
    }
}

/// Installs, once per process, the thread that handles SIGINT, SIGTERM and
/// SIGHUP: it records the signal, then closes the current connection (the
/// daemon cancels the job), then exits with 128 + signal. The record comes
/// first, so the main thread, which sees the connection close, never takes
/// that for a daemon upgrade and never sends the job again.
fn handle_signals() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGINT, SIGTERM, SIGHUP]) else {
            return;
        };
        std::thread::spawn(move || {
            if let Some(sig) = signals.forever().next() {
                SIGNALLED.store(sig, Ordering::SeqCst);
                if let Some(close) = CLOSER.lock().ok().as_mut().and_then(|c| c.as_mut()) {
                    close();
                }
                std::process::exit(128 + sig);
            }
        });
    });
}

/// Client settings, read from `SLOT_*` variables.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// Remote ssh host; `None` = local daemon.
    pub host: Option<String>,
    /// ssh command and options.
    pub ssh: Vec<String>,
    /// Remote workspace root.
    pub ws_root: String,
    /// slot executable on the remote host.
    pub remote_slot: String,
    /// Extra variable names forwarded to a remote host.
    pub extra_env: HashSet<String>,
    /// Local daemon socket.
    pub socket: PathBuf,
    /// `--fetch` paths, relative to the repository root, copied back from
    /// the server after a remote job exits 0.
    pub fetch: Vec<String>,
    /// `--tokens N` or `SLOT_TOKENS`: slots the job holds in place of its
    /// toolchain's count; the daemon caps it.
    pub tokens: Option<u32>,
    /// `--priority <level>` or `SLOT_PRIORITY`: the job's own queue level;
    /// a rule of the daemon's table for the repository wins over it.
    pub priority: Option<crate::types::Priority>,
    /// `--timeout <duration>` or `SLOT_TIMEOUT`: seconds the job may run in
    /// place of the daemon's run-time limit; the daemon caps it.
    pub timeout: Option<u64>,
    /// `SLOT_LOCAL_FALLBACK` set and not `0`: with no daemon answering, run
    /// the command here, outside the pool, in place of exiting with
    /// [`NOT_RUN`].
    pub fallback: bool,
    /// `SLOT_SYNC_STALL` and `SLOT_SYNC_LIMIT`: when a remote sync is
    /// killed and the job runs on this machine's daemon instead.
    pub sync: sync::Bounds,
    /// `SLOT_REMOTE_SAME_PATH` set and not `0`: the server mirrors the
    /// tree at its absolute path here, and the job runs there.
    pub same_path: bool,
    /// `--here`: the job goes to this machine's daemon whatever `SLOT_HOST`
    /// says, and carries the reason [`crate::server::spill::HERE`], so the
    /// daemon never hands it to a node.
    pub here: bool,
}

/// A duration from variable `name`, or `default` with a line on stderr when
/// it does not parse.
fn duration_var(name: &str, default: Duration) -> Duration {
    match std::env::var(name).ok().filter(|v| !v.is_empty()) {
        None => default,
        Some(v) => crate::run_limit::parse_duration(&v).map_or_else(
            |e| {
                eprintln!("slot: {name}: {e}; using {} s", default.as_secs());
                default
            },
            Duration::from_secs,
        ),
    }
}

impl Settings {
    /// Reads `SLOT_HOST`, `SLOT_SSH`, `SLOT_WSROOT`, `SLOT_REMOTE`,
    /// `SLOT_ENV`, `SLOT_SOCKET`, `SLOT_LOCAL_FALLBACK`, `SLOT_SYNC_STALL`,
    /// `SLOT_SYNC_LIMIT` and `SLOT_REMOTE_SAME_PATH`.
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let ssh = var("SLOT_SSH").map_or_else(
            || {
                let runtime =
                    paths::runtime_dir().map_or_else(|| ".".into(), |d| d.display().to_string());
                [
                    "ssh",
                    "-o",
                    "ControlMaster=auto",
                    "-o",
                    "ControlPersist=10m",
                    "-o",
                    &format!("ControlPath={runtime}/slot-ssh-%C"),
                    "-o",
                    "ConnectTimeout=3",
                ]
                .iter()
                .map(|s| (*s).to_string())
                .collect()
            },
            |s| s.split_whitespace().map(String::from).collect(),
        );
        Self {
            host: var("SLOT_HOST"),
            ssh,
            ws_root: var("SLOT_WSROOT").unwrap_or_else(|| paths::DEFAULT_REMOTE_WSROOT.into()),
            remote_slot: var("SLOT_REMOTE").unwrap_or_else(|| "slot".into()),
            extra_env: var("SLOT_ENV")
                .map(|s| {
                    s.split(',')
                        .map(|x| x.trim().to_string())
                        .filter(|x| !x.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            socket: var("SLOT_SOCKET").map_or_else(paths::default_socket, PathBuf::from),
            fetch: Vec::new(),
            tokens: None,
            priority: None,
            timeout: None,
            fallback: var("SLOT_LOCAL_FALLBACK").is_some_and(|v| v != "0"),
            same_path: var("SLOT_REMOTE_SAME_PATH").is_some_and(|v| v != "0"),
            here: false,
            sync: sync::Bounds {
                stall: duration_var("SLOT_SYNC_STALL", sync::DEFAULT_STALL),
                limit: duration_var("SLOT_SYNC_LIMIT", sync::DEFAULT_LIMIT),
            },
        }
    }
}

/// Runs `argv` through the daemon and returns the exit code to use.
pub fn run(argv: &[String], s: &Settings) -> i32 {
    if nested() {
        return run_local(argv);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let CheckoutInfo {
        top,
        git_dirs,
        repo,
        checkout,
    } = checkout_info(&cwd);
    let mut req = new_request(argv, s, repo, checkout);
    // The never-sync list holds whatever `SLOT_HOST` says: its reason also
    // keeps the daemon from handing the job to a node.
    let never = never_sync::of_job(&never_sync::Job {
        cwd: &cwd,
        top: top.as_deref(),
        git_dirs: &git_dirs,
        argv,
    });
    if let Some(reason) = &never {
        req.local_reason.clone_from(reason);
    }
    if let Some(dir) = crate::gate::dir_from_env() {
        local_fields(&mut req, &cwd);
        return crate::gate::submit(&dir, &req);
    }
    if let (Some(reason), Some(host), false) = (&never, &s.host, s.here) {
        if std::env::var_os(crate::server::spill::SPILLED_ENV).is_some_and(|v| !v.is_empty()) {
            eprintln!("slot: {reason}: the tree is not synced to {host}");
            return crate::server::spill::INFRA_EXIT;
        }
        eprintln!("slot: {reason}: the tree is not synced to {host}; running on this machine");
    }
    let host = if never.is_some() {
        None
    } else {
        remote_host(&mut req, s, || listed_local(top.as_deref(), argv))
    };
    if let Some(host) = host {
        let plan = artifacts::plan(argv);
        if let Some(p) = &plan {
            req.argv.clone_from(&p.argv);
        }
        req.fetch_once = crate::cargo_fetch::applies(argv);
        // A same-path job outside any git checkout mirrors its working
        // directory as the tree.
        let top = top.or_else(|| s.same_path.then(|| cwd.clone()));
        let why = match remote(host, top.as_deref(), &cwd, &mut req, s) {
            Ok(c) => {
                let mut collector = plan.as_ref().map(|p| artifacts::Collector::new(p.strip));
                match talk_with(c, &req, collector.as_mut()) {
                    Ended::Fallback(msg) => msg,
                    ended => {
                        let ran = matches!(ended, Ended::Exit(_));
                        let mut code = ended.code();
                        if let (Some(p), Some(col), Some(top)) = (&plan, &collector, top.as_deref())
                            && ran
                            && (code == 0 || p.when == artifacts::When::AnyExit)
                        {
                            code = products_back(host, top, &cwd, s, col, code);
                        }
                        return match (&req.workspace, top.as_deref()) {
                            (Some(ws), Some(top)) if code == 0 && !s.fetch.is_empty() => {
                                fetch(host, top, &ws.id, s)
                            }
                            _ => code,
                        };
                    }
                }
            }
            Err(e) => format!("{e:#}"),
        };
        // A job the daemon handed to a node goes back to the daemon's queue.
        if std::env::var_os(crate::server::spill::SPILLED_ENV).is_some_and(|v| !v.is_empty()) {
            eprintln!("slot: remote build on {host} did not start ({why})");
            return crate::server::spill::INFRA_EXIT;
        }
        eprintln!(
            "slot: remote build on {host} did not start ({why}); running it on this machine's daemon"
        );
        req.argv = argv.to_vec();
        req.workspace = None;
        req.fetch_once = false;
        req.local_reason = sync_failed(&why);
        local_fields(&mut req, &cwd);
        return submit_local(&s.socket, &req, &Waits::of(s));
    }
    local_fields(&mut req, &cwd);
    submit_local(&s.socket, &req, &Waits::of(s))
}

/// The request for `argv` before its route is known. With `--here` it
/// carries the reason [`crate::server::spill::HERE`], so the daemon never
/// hands the job to a node.
fn new_request(
    argv: &[String],
    s: &Settings,
    repo: Option<String>,
    checkout: Option<Checkout>,
) -> Request {
    Request {
        argv: argv.to_vec(),
        session: std::env::var("CLAUDE_CODE_SESSION_ID").unwrap_or_default(),
        client: client_id(),
        repo: repo.unwrap_or_default(),
        checkout,
        tokens: s.tokens,
        priority: s.priority,
        timeout_s: s.timeout,
        local_reason: if s.here {
            crate::server::spill::HERE.into()
        } else {
            String::new()
        },
        ..Request::default()
    }
}

/// The build server the job goes to, or `None` when it stays on this
/// machine's daemon: no `SLOT_HOST`, `--here`, or a command `.slot.toml`
/// lists as local (`listed`, asked only when a host is set and `--here` is
/// not), which also names the job's reason.
fn remote_host<'a>(
    req: &mut Request,
    s: &'a Settings,
    listed: impl FnOnce() -> bool,
) -> Option<&'a String> {
    let host = s.host.as_ref()?;
    if s.here {
        return None;
    }
    if listed() {
        req.local_reason = crate::server::spill::LOCAL_REQUESTED.into();
        return None;
    }
    Some(host)
}

/// The `local_reason` of a job whose remote start failed with `why`:
/// `sync failed: ` and the first line of the error.
pub fn sync_failed(why: &str) -> String {
    let first = crate::server::spill::first_line(why);
    if first.is_empty() {
        "sync failed".into()
    } else {
        format!("sync failed: {first}")
    }
}

/// What only a daemon on this machine uses: the working directory, the whole
/// environment and this process's open-file limit, which the job gets. A
/// remote request has a filtered environment instead and no limit: the
/// build server's applies.
fn local_fields(req: &mut Request, cwd: &Path) {
    req.cwd = cwd.to_string_lossy().into_owned();
    req.env = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
    req.nofile = Some(crate::proto::FileLimit::of_this_process());
}

/// The job's target directory on the server, from its `start` frame.
static JOB_TARGET: Mutex<String> = Mutex::new(String::new());

/// Where `slot --fetch <path>` reads `path` on the server: under the job's
/// target directory for `target` and `target/…`, under the workspace mirror
/// for anything else. `None` with the reason when the path cannot be fetched.
pub fn fetch_source(path: &str, mirror: &str, target_dir: &str) -> Result<String, String> {
    let p = Path::new(path);
    if path.is_empty() || p.is_absolute() {
        return Err(format!(
            "`{path}`: name a path relative to the repository root"
        ));
    }
    if p.components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(format!("`{path}`: `.` and `..` are not allowed"));
    }
    let mut parts = p.components();
    let first = parts
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned());
    if first.as_deref() == Some("target") {
        if target_dir.is_empty() {
            return Err(format!(
                "`{path}`: the server did not report its target directory (slot 0.8.0 or later needed there)"
            ));
        }
        let rest = parts.as_path().to_string_lossy().into_owned();
        return Ok(if rest.is_empty() {
            target_dir.to_string()
        } else {
            format!("{target_dir}/{rest}")
        });
    }
    Ok(format!("{mirror}/{path}"))
}

/// Copies every `--fetch` path back from `host` into the repository at
/// `top`. Returns 0, or 1 after printing which path failed.
fn fetch(host: &str, top: &Path, id: &str, s: &Settings) -> i32 {
    let target_dir = JOB_TARGET.lock().map(|t| t.clone()).unwrap_or_default();
    let mirror = if s.same_path {
        top.to_string_lossy().into_owned()
    } else {
        format!("{}/{id}", s.ws_root)
    };
    let mut code = 0;
    for path in &s.fetch {
        let src = match fetch_source(path, &mirror, &target_dir) {
            Ok(src) => src,
            Err(e) => {
                eprintln!("slot: fetch {e}");
                code = 1;
                continue;
            }
        };
        let dst = top.join(path);
        let parent = dst.parent().unwrap_or(top);
        let st = Command::new("rsync")
            .args(["-a", "--mkpath", "-e"])
            .arg(s.ssh.join(" "))
            .arg(format!("{host}:{src}"))
            .arg(format!("{}/", parent.display()))
            .status();
        match st {
            Ok(st) if st.success() => eprintln!("slot: fetched {path}"),
            Ok(st) => {
                eprintln!("slot: fetch `{path}`: rsync exited with {st}");
                code = 1;
            }
            Err(e) => {
                eprintln!("slot: fetch `{path}`: rsync: {e}");
                code = 1;
            }
        }
    }
    code
}

/// Copies the products `col` collected from the job's target directory on
/// `host` into the local target directory and prints one line. Returns
/// `code`, or 1 when the copy failed after a job that exited 0: the caller
/// would run a stale binary.
fn products_back(
    host: &str,
    top: &Path,
    cwd: &Path,
    s: &Settings,
    col: &artifacts::Collector,
    code: i32,
) -> i32 {
    let target_dir = JOB_TARGET.lock().map(|t| t.clone()).unwrap_or_default();
    if target_dir.is_empty() {
        eprintln!(
            "slot: artifacts not fetched: the server did not report its target directory (slot 0.8.0 or later needed there)"
        );
        return code;
    }
    let rels: Vec<String> = col
        .paths()
        .iter()
        .filter_map(|p| artifacts::relative(p, &target_dir))
        .collect();
    if rels.is_empty() {
        return code;
    }
    // A same-path job built into the client's own target directory, at the
    // same path on the server.
    let local = if s.same_path {
        PathBuf::from(&target_dir)
    } else {
        artifacts::local_target(cwd, top, std::env::var("CARGO_TARGET_DIR").ok().as_deref())
    };
    let src = artifacts::Source {
        host,
        ssh: &s.ssh,
        target: &target_dir,
    };
    match artifacts::fetch(&src, &rels, &local, &s.sync) {
        Ok(f) => {
            eprintln!("{}", f.line());
            code
        }
        Err(e) => {
            eprintln!("slot: artifacts not fetched: {e:#}");
            if code == 0 { 1 } else { code }
        }
    }
}

/// Whether the repository's `.slot.toml` lists `argv` under `local`, so it
/// runs on this machine's daemon although `SLOT_HOST` is set. Says so on
/// stderr; a `.slot.toml` that does not parse is reported and ignored.
fn listed_local(top: Option<&Path>, argv: &[String]) -> bool {
    let Some(top) = top else { return false };
    match RepoConfig::load(top) {
        Ok(c) => c.local_match(argv).is_some_and(|p| {
            eprintln!(
                "slot: {} lists `{p}` as local: running on this machine",
                repo_config::FILE
            );
            true
        }),
        Err(e) => {
            eprintln!("slot: {e}; ignored");
            false
        }
    }
}

/// `slot cancel <id>`: asks the daemon to cancel job `id`. With `SLOT_HOST`
/// set, runs `slot cancel` on that host over ssh.
pub fn cancel(id: i64, s: &Settings) -> i32 {
    if let Some(host) = &s.host {
        let Some((ssh, args)) = s.ssh.split_first() else {
            eprintln!("slot: SLOT_SSH is empty");
            return 2;
        };
        return match Command::new(ssh)
            .args(args)
            .arg(host)
            .arg(&s.remote_slot)
            .arg("cancel")
            .arg(id.to_string())
            .status()
        {
            Ok(st) => st.code().unwrap_or(1),
            Err(e) => {
                eprintln!("slot: {ssh}: {e}");
                3
            }
        };
    }
    let req = Request {
        cancel: Some(id),
        ..Request::default()
    };
    match local(&s.socket) {
        Ok(c) => talk(c, &req).code(),
        Err(e) => {
            eprintln!("slot: daemon unreachable ({e:#})");
            3
        }
    }
}

/// How long a call waits for a daemon whose socket does not answer, as
/// during a restart, before it gives up.
pub const DAEMON_WAIT: Duration = Duration::from_secs(60);

/// Exit code of a call whose command did not run: no daemon answered.
pub const NOT_RUN: i32 = 125;

/// What a call does while the daemon does not answer.
#[derive(Debug, Clone, Copy)]
pub struct Waits {
    /// How long a new call waits for the socket.
    pub absent: Duration,
    /// How long a job dropped by an upgrading daemon waits for it.
    pub upgrade: Duration,
    /// `SLOT_LOCAL_FALLBACK`: run the command here, outside the pool, when
    /// the daemon does not answer, in place of exiting with [`NOT_RUN`].
    pub fallback: bool,
}

impl Waits {
    /// The waits of a call from the command line.
    pub fn of(s: &Settings) -> Self {
        Self {
            absent: DAEMON_WAIT,
            upgrade: UPGRADE_WAIT,
            fallback: s.fallback,
        }
    }
}

/// Why no connection came within a wait.
enum Gone {
    /// The client got this signal.
    Signal(i32),
    /// The wait ended; the last error.
    Timeout(anyhow::Error),
}

/// Connects to `socket`, trying again until `wait` has passed.
fn connect_within(socket: &Path, wait: Duration) -> Result<Conn, Gone> {
    let deadline = Instant::now() + wait;
    loop {
        if let Some(sig) = signalled() {
            return Err(Gone::Signal(sig));
        }
        match local(socket) {
            Ok(c) => return Ok(c),
            Err(_) if Instant::now() < deadline => std::thread::sleep(UPGRADE_POLL),
            Err(e) => return Err(Gone::Timeout(e)),
        }
    }
}

/// The end of a call no daemon answered within `wait`: run here with the
/// opt-in, else not at all.
fn unanswered(req: &Request, w: &Waits, wait: Duration, e: &anyhow::Error) -> i32 {
    if w.fallback {
        eprintln!(
            "slot: daemon not answering after {}s ({e:#}), ran locally (SLOT_LOCAL_FALLBACK)",
            wait.as_secs()
        );
        return run_local(&req.argv);
    }
    eprintln!(
        "slot: no daemon answered within {}s ({e:#}); the command did not run. \
         `slot --local …` or SLOT_LOCAL_FALLBACK=1 runs it outside the pool",
        wait.as_secs()
    );
    NOT_RUN
}

/// Runs `req` through the daemon at `socket`. With no daemon answering, the
/// call waits for the socket for up to `w.absent`, then exits with
/// [`NOT_RUN`] without running the command, unless `w.fallback` is set:
/// then it runs here at once, outside the pool. An upgrading daemon drops
/// the connections of jobs it has not started and re-executes; such a job
/// is sent again, once, as soon as the socket is back, for up to
/// `w.upgrade`. A job whose process had started is not sent again, nor one
/// whose client got a signal: the signal cancelled it.
pub fn submit_local(socket: &Path, req: &Request, w: &Waits) -> i32 {
    let conn = match local(socket) {
        Ok(c) => c,
        Err(e) if w.fallback => {
            eprintln!("slot: daemon unreachable ({e:#}), ran locally (SLOT_LOCAL_FALLBACK)");
            return run_local(&req.argv);
        }
        Err(e) => {
            eprintln!(
                "slot: daemon unreachable ({e:#}); waiting up to {}s for it",
                w.absent.as_secs()
            );
            match connect_within(socket, w.absent) {
                Ok(c) => c,
                Err(Gone::Signal(sig)) => return 128 + sig,
                Err(Gone::Timeout(e)) => return unanswered(req, w, w.absent, &e),
            }
        }
    };
    match talk(conn, req) {
        Ended::Lost { started: false } => {}
        ended => return ended.code(),
    }
    if let Some(sig) = signalled() {
        return 128 + sig;
    }
    eprintln!("slot: daemon upgrading, resubmitting job");
    match connect_within(socket, w.upgrade) {
        Ok(c) => talk(c, req).code(),
        Err(Gone::Signal(sig)) => 128 + sig,
        Err(Gone::Timeout(e)) => unanswered(req, w, w.upgrade, &e),
    }
}

/// Whether this client runs inside a slot job. Such a call runs directly:
/// it inherits the outer job's jobserver, so it shares the pool without
/// queueing a second job that could wait on slots its parent holds.
pub fn nested() -> bool {
    std::env::var_os(crate::server::JOB_ENV).is_some_and(|v| !v.is_empty())
}

/// Runs `argv` here, without the daemon.
pub fn run_local(argv: &[String]) -> i32 {
    match Command::new(&argv[0]).args(&argv[1..]).status() {
        Ok(st) => st.code().unwrap_or_else(|| 128 + st.signal().unwrap_or(0)),
        Err(e) => {
            eprintln!("slot: {}: {e}", argv[0]);
            127
        }
    }
}

fn hostname() -> String {
    nix::unistd::gethostname()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_default()
}

fn client_id() -> String {
    format!("{}:{}", hostname(), nix::unistd::getppid())
}

/// Repository root and name of the git checkout containing `cwd`. A linked
/// worktree reports its main repository's name.
pub fn git_info(cwd: &Path) -> (Option<PathBuf>, Option<String>) {
    let info = checkout_info(cwd);
    (info.top, info.repo)
}

#[derive(Default)]
struct CheckoutInfo {
    top: Option<PathBuf>,
    /// The git directory and the common one (they differ in a linked
    /// worktree).
    git_dirs: Vec<PathBuf>,
    repo: Option<String>,
    checkout: Option<Checkout>,
}

fn checkout_info(cwd: &Path) -> CheckoutInfo {
    let Ok(out) = Command::new("git")
        .env("LC_ALL", "C")
        .arg("-C")
        .arg(cwd)
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
        ])
        .output()
    else {
        return CheckoutInfo::default();
    };
    if !out.status.success() {
        // Git also exits 128 for a broken checkout or permission problem.
        let outside = out
            .stderr
            .starts_with(b"fatal: not a git repository (or any of the parent directories)")
            || out
                .stderr
                .starts_with(b"fatal: not a git repository (or any parent up to mount point ");
        return CheckoutInfo {
            checkout: outside.then_some(Checkout::Directory),
            ..CheckoutInfo::default()
        };
    }
    let mut paths = out
        .stdout
        .split(|byte| *byte == b'\n')
        .map(OsStr::from_bytes)
        .map(Path::new);
    let (Some(top), Some(git_dir), Some(common)) = (paths.next(), paths.next(), paths.next())
    else {
        return CheckoutInfo::default();
    };
    let name = if common.file_name().is_some_and(|n| n == ".git") {
        common.parent().and_then(Path::file_name)
    } else {
        common.file_name()
    };
    let repo = name.map(|n| n.to_string_lossy().trim_end_matches(".git").to_string());
    let checkout = if git_dir == common {
        Some(Checkout::Repository)
    } else {
        top.file_name().map(|name| Checkout::Worktree {
            name: name.to_string_lossy().into_owned(),
        })
    };
    CheckoutInfo {
        top: Some(top.to_path_buf()),
        git_dirs: vec![git_dir.to_path_buf(), common.to_path_buf()],
        repo,
        checkout,
    }
}

/// Workspace id for a repository root on this host.
pub fn workspace_id(top: &Path) -> String {
    let digest = Sha256::digest(format!("{}:{}", hostname(), top.display()).as_bytes());
    digest.iter().take(8).fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// A connection to the daemon, split into halves.
pub struct Conn {
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    closer: Box<dyn FnMut() + Send>,
}

fn local(socket: &Path) -> Result<Conn> {
    let c = UnixStream::connect(socket).with_context(|| format!("connect {}", socket.display()))?;
    c.set_write_timeout(Some(Duration::from_secs(2)))?;
    let reader = c.try_clone()?;
    let writer = c.try_clone()?;
    Ok(Conn {
        reader: Box::new(reader),
        writer: Box::new(writer),
        closer: Box::new(move || {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }),
    })
}

/// Syncs what a same-path job needs beside its tree at `top`: the git
/// directories (a failed sync is reported, and the job runs without
/// them), and the working directory when the tree's sync left it out (a
/// failed sync is the caller's error: the job could not start there).
fn same_path_extras(
    top: &Path,
    cwd: &Path,
    excludes: &[&str],
    r: &sync::Remote<'_>,
    s: &Settings,
) -> Result<()> {
    use same_path::{Extra, Kind};
    for path in same_path::git_dirs(top) {
        let e = Extra {
            path,
            kind: Kind::Git,
        };
        if let Err(err) = sync::sync_extra(&e, r, &s.sync) {
            eprintln!(
                "slot: git directory {} not synced: {err:#}",
                e.path.display()
            );
        }
    }
    if same_path::cwd_ignored(top, cwd, excludes) {
        let e = Extra {
            path: cwd.to_path_buf(),
            kind: Kind::Tree,
        };
        sync::sync_extra(&e, r, &s.sync)
            .with_context(|| format!("sync of the working directory {}", cwd.display()))?;
    }
    Ok(())
}

fn remote(
    host: &str,
    top: Option<&Path>,
    cwd: &Path,
    req: &mut Request,
    s: &Settings,
) -> Result<Conn> {
    let top = top.ok_or_else(|| anyhow!("remote mode needs a git repository"))?;
    let id = workspace_id(top);
    let rel = cwd
        .strip_prefix(top)
        .unwrap_or(Path::new(""))
        .to_string_lossy()
        .into_owned();
    let tc = toolchain::for_argv(&req.argv, 1);
    let target = sync::Remote {
        host,
        ssh: &s.ssh,
        ws_root: &s.ws_root,
        remote_slot: &s.remote_slot,
        same_path: s.same_path,
    };
    let synced = sync::sync(top, &id, tc.sync_excludes(), &target, &s.sync)?;
    eprintln!("{}", synced.line(host));
    let target_dir = if s.same_path {
        same_path_extras(top, cwd, tc.sync_excludes(), &target, s)?;
        same_path::target_dir(
            &req.argv,
            cwd,
            top,
            std::env::var("CARGO_TARGET_DIR").ok().as_deref(),
        )
        .map(|t| t.to_string_lossy().into_owned())
        .unwrap_or_default()
    } else {
        String::new()
    };
    let (sent, held) = toolchain::remote_env(
        std::env::vars(),
        |k| tc.client_env(k),
        |k| s.extra_env.contains(k),
    );
    req.env.extend(sent);
    if !held.is_empty() {
        eprintln!(
            "slot: not sent to {host}, the names look like a credential: {}",
            held.join(", ")
        );
    }
    req.workspace = Some(Workspace {
        id,
        rel_cwd: rel,
        client_root: top.to_string_lossy().into_owned(),
        same_path: s.same_path,
        target_dir,
    });
    let mut child: Child = Command::new(&s.ssh[0])
        .args(&s.ssh[1..])
        .arg(host)
        .arg(&s.remote_slot)
        .arg("proxy")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("ssh")?;
    let writer = child.stdin.take().ok_or_else(|| anyhow!("ssh stdin"))?;
    let reader = child.stdout.take().ok_or_else(|| anyhow!("ssh stdout"))?;
    Ok(Conn {
        reader: Box::new(reader),
        writer: Box::new(writer),
        closer: Box::new(move || {
            let _ = child.kill();
        }),
    })
}

/// How a conversation with the daemon ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ended {
    /// The job's exit code, or 3 when the request could not be sent.
    Exit(i32),
    /// The connection closed without an exit frame. `started` is whether
    /// the job's process had started (a start, output or error frame came).
    Lost { started: bool },
    /// The client got this signal; the job was cancelled.
    Signalled(i32),
    /// The server sent the job back without starting it, for this reason.
    Fallback(String),
}

impl Ended {
    /// The exit code for the caller; a lost connection is reported here.
    fn code(self) -> i32 {
        match self {
            Self::Exit(code) => code,
            Self::Signalled(sig) => 128 + sig,
            Self::Lost { .. } => {
                eprintln!("slot: connection to daemon lost");
                3
            }
            Self::Fallback(msg) => {
                eprintln!("slot: the job did not start: {msg}");
                3
            }
        }
    }
}

fn talk(conn: Conn, req: &Request) -> Ended {
    talk_with(conn, req, None)
}

/// Talks to the daemon; the job's stdout goes through `collector` when
/// given.
fn talk_with(conn: Conn, req: &Request, mut collector: Option<&mut artifacts::Collector>) -> Ended {
    let Conn {
        reader,
        mut writer,
        closer,
    } = conn;
    let mut line = match serde_json::to_vec(req) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("slot: {e}");
            return Ended::Exit(3);
        }
    };
    line.push(b'\n');
    if let Err(e) = writer.write_all(&line).and_then(|()| writer.flush()) {
        eprintln!("slot: send: {e}");
        return Ended::Exit(3);
    }
    // The writer stays open until the job ends: closing it tells the daemon
    // the client is gone, and the daemon cancels the job.
    if let Ok(mut c) = CLOSER.lock() {
        *c = Some(closer);
    }
    handle_signals();
    if let Some(sig) = signalled() {
        return Ended::Signalled(sig);
    }
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let mut started = false;
    for l in BufReader::new(reader).lines() {
        let Ok(l) = l else { break };
        match serde_json::from_str::<Frame>(&l) {
            Ok(Frame::Out { b }) => {
                started = true;
                match collector.as_deref_mut() {
                    Some(c) => {
                        let _ = stdout.write_all(&c.push(b.as_bytes()));
                    }
                    None => {
                        let _ = stdout.write_all(b.as_bytes());
                    }
                }
                let _ = stdout.flush();
            }
            Ok(Frame::Err { b }) => {
                started = true;
                let _ = stderr.write_all(b.as_bytes());
            }
            Ok(Frame::Start { target_dir, .. }) => {
                started = true;
                if let Ok(mut t) = JOB_TARGET.lock() {
                    *t = target_dir;
                }
            }
            Ok(Frame::Info { msg }) => eprintln!("{msg}"),
            Ok(Frame::Exit { code }) => {
                drop(writer);
                if let Some(c) = collector.as_deref_mut() {
                    let _ = stdout.write_all(&c.finish());
                    let _ = stdout.flush();
                }
                return Ended::Exit(code);
            }
            Ok(Frame::Fallback { msg }) if !started => {
                drop(writer);
                return Ended::Fallback(msg);
            }
            Ok(Frame::Fallback { .. }) | Err(_) => {}
        }
    }
    if let Some(c) = collector {
        let _ = stdout.write_all(&c.finish());
        let _ = stdout.flush();
    }
    if let Some(sig) = signalled() {
        return Ended::Signalled(sig);
    }
    Ended::Lost { started }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;

    #[test]
    fn here_names_its_reason_on_the_request() {
        let argv = vec!["git".to_string(), "fetch".to_string()];
        let here = Settings {
            here: true,
            ..Settings::default()
        };
        let req = new_request(&argv, &here, None, None);
        assert_eq!(req.local_reason, "here: needs this host");
        assert_eq!(
            crate::server::spill::local_reason(&req, false),
            "here: needs this host"
        );
        assert_eq!(
            crate::server::spill::refusal(&req).as_deref(),
            Some("here: needs this host")
        );
        let plain = new_request(&argv, &Settings::default(), None, None);
        assert_eq!(plain.local_reason, "");
    }

    #[test]
    fn here_skips_remote_mode() {
        let argv = vec!["git".to_string(), "push".to_string()];
        let remote = Settings {
            host: Some("build-1".into()),
            ..Settings::default()
        };
        let here = Settings {
            here: true,
            ..remote.clone()
        };
        let mut req = new_request(&argv, &here, None, None);
        let asked = std::cell::Cell::new(false);
        let to = remote_host(&mut req, &here, || {
            asked.set(true);
            true
        });
        assert_eq!(to, None);
        assert!(!asked.get(), ".slot.toml is not read with --here");
        assert_eq!(req.local_reason, "here: needs this host");

        let mut req = new_request(&argv, &remote, None, None);
        assert_eq!(
            remote_host(&mut req, &remote, || false).map(String::as_str),
            Some("build-1")
        );
        assert_eq!(req.local_reason, "");
        assert_eq!(remote_host(&mut req, &remote, || true), None);
        assert_eq!(req.local_reason, "local requested");

        let mut req = new_request(&argv, &Settings::default(), None, None);
        assert_eq!(remote_host(&mut req, &Settings::default(), || true), None);
        assert_eq!(req.local_reason, "");
    }

    #[test]
    fn a_failed_remote_start_names_its_first_error_line() {
        assert_eq!(
            sync_failed("rsync: connection unexpectedly closed\ncode 12"),
            "sync failed: rsync: connection unexpectedly closed"
        );
        assert_eq!(
            sync_failed("\n  ssh: timeout  \n"),
            "sync failed: ssh: timeout"
        );
        assert_eq!(sync_failed(""), "sync failed");
    }

    #[test]
    fn fetch_source_maps_target_to_the_job_target_dir() {
        let t = "/srv/cache/target/abc";
        let m = "ws/abc";
        assert_eq!(
            fetch_source("target/debug/ess", m, t),
            Ok("/srv/cache/target/abc/debug/ess".into())
        );
        assert_eq!(fetch_source("target", m, t), Ok(t.into()));
        assert_eq!(fetch_source("bin/tool", m, t), Ok("ws/abc/bin/tool".into()));
        assert_eq!(
            fetch_source("targets/x", m, t),
            Ok("ws/abc/targets/x".into())
        );
        for bad in ["", "/etc/passwd", "../x", "target/../../x", "./x"] {
            assert!(fetch_source(bad, m, t).is_err(), "{bad}");
        }
        let old = fetch_source("target/debug/ess", m, "").unwrap_err();
        assert!(old.contains("0.8.0"), "{old}");
    }

    /// A fake daemon: for each script, accepts one connection, reads the
    /// request, reports it, writes the script's frames and closes. Between
    /// connections the socket is gone for `gap`, as during a re-exec.
    fn fake_daemon(
        socket: PathBuf,
        scripts: Vec<Vec<Frame>>,
        gap: Duration,
    ) -> mpsc::Receiver<Request> {
        let (tx, rx) = mpsc::channel();
        let l = UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            let mut l = Some(l);
            for (i, frames) in scripts.into_iter().enumerate() {
                if i > 0 {
                    std::thread::sleep(gap);
                    l = Some(UnixListener::bind(&socket).unwrap());
                }
                let (c, _) = l.as_ref().unwrap().accept().unwrap();
                let mut line = String::new();
                BufReader::new(&c).read_line(&mut line).unwrap();
                tx.send(serde_json::from_str(&line).unwrap()).unwrap();
                let mut w = &c;
                for f in frames {
                    let mut b = serde_json::to_vec(&f).unwrap();
                    b.push(b'\n');
                    w.write_all(&b).unwrap();
                }
                drop(l.take());
                let _ = std::fs::remove_file(&socket);
                drop(c);
            }
        });
        rx
    }

    fn request() -> Request {
        Request {
            argv: vec!["cargo".into(), "check".into()],
            cwd: "/w".into(),
            ..Request::default()
        }
    }

    /// No wait for an absent daemon, `upgrade` for an upgrading one, no
    /// local run.
    fn waits(upgrade: Duration) -> Waits {
        Waits {
            absent: Duration::ZERO,
            upgrade,
            fallback: false,
        }
    }

    fn git_setup(cwd: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgSign=false",
                "-c",
                "user.name=Slot Test",
                "-c",
                "user.email=slot@example.invalid",
                "-c",
                "protocol.file.allow=always",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn checkout_identity_follows_git_from_inside_a_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().canonicalize().unwrap().join("main-repo");
        let outside = dir.path().join("scratch/.sdc/impl-gate");
        let inside = repo.join(".sdc/impl-gate");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(&inside).unwrap();
        git_setup(&repo, &["init", "-q"]);

        let info = checkout_info(&outside);
        assert!(matches!(info.checkout, Some(Checkout::Directory)));
        assert_eq!(git_info(&outside), (None, None));

        assert!(matches!(
            checkout_info(&inside).checkout,
            Some(Checkout::Repository)
        ));
        assert_eq!(git_info(&inside), (Some(repo), Some("main-repo".into())));
    }

    #[test]
    fn detached_linked_checkout_keeps_the_common_repository_name() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = root.join("main-repo");
        let linked = root.join("linked-checkout");
        std::fs::create_dir(&repo).unwrap();
        git_setup(&repo, &["init", "-q"]);
        git_setup(&repo, &["commit", "--allow-empty", "-qm", "initial"]);
        git_setup(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                linked.to_str().unwrap(),
            ],
        );
        let cwd = linked.join("nested");
        std::fs::create_dir(&cwd).unwrap();

        assert!(matches!(
            checkout_info(&cwd).checkout,
            Some(Checkout::Worktree { name }) if name == "linked-checkout"
        ));
        assert_eq!(git_info(&cwd), (Some(linked), Some("main-repo".into())));
    }

    #[test]
    fn a_submodule_is_a_repository_with_its_own_git_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = root.join("main-repo");
        let source = root.join("component-repo");
        std::fs::create_dir(&repo).unwrap();
        std::fs::create_dir(&source).unwrap();
        git_setup(&source, &["init", "-q"]);
        git_setup(&source, &["commit", "--allow-empty", "-qm", "initial"]);
        git_setup(&repo, &["init", "-q"]);
        git_setup(
            &repo,
            &[
                "submodule",
                "add",
                "-q",
                source.to_str().unwrap(),
                "vendor/component",
            ],
        );
        let submodule = repo.join("vendor/component");
        let cwd = submodule.join("nested");
        std::fs::create_dir(&cwd).unwrap();

        assert!(matches!(
            checkout_info(&cwd).checkout,
            Some(Checkout::Repository)
        ));
        assert_eq!(git_info(&cwd), (Some(submodule), Some("component".into())));
    }

    #[test]
    fn a_broken_git_directory_has_unknown_identity() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".git"), "gitdir: missing\n").unwrap();

        assert!(checkout_info(dir.path()).checkout.is_none());
        assert_eq!(git_info(dir.path()), (None, None));
    }

    #[test]
    fn a_job_dropped_while_queued_is_resubmitted_once() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s");
        let queued = Frame::Info {
            msg: "slot: job 1 queued".into(),
        };
        let rx = fake_daemon(
            socket.clone(),
            vec![
                vec![queued],
                vec![
                    Frame::Start {
                        id: 1,
                        target_dir: String::new(),
                    },
                    Frame::Exit { code: 7 },
                ],
            ],
            Duration::from_millis(800),
        );
        let t0 = Instant::now();
        assert_eq!(
            submit_local(&socket, &request(), &waits(Duration::from_secs(10))),
            7
        );
        assert!(t0.elapsed() >= Duration::from_millis(800));
        assert_eq!(rx.recv().unwrap(), request());
        assert_eq!(rx.recv().unwrap(), request(), "the same request again");
    }

    #[test]
    fn a_started_job_is_not_resubmitted() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s");
        let rx = fake_daemon(
            socket.clone(),
            vec![
                vec![Frame::Start {
                    id: 1,
                    target_dir: String::new(),
                }],
                vec![Frame::Exit { code: 0 }],
            ],
            Duration::from_millis(100),
        );
        assert_eq!(
            submit_local(&socket, &request(), &waits(Duration::from_secs(10))),
            3
        );
        assert_eq!(rx.recv().unwrap(), request());
        assert!(
            rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "no second submission"
        );
    }

    #[test]
    fn output_from_an_older_daemon_counts_as_started() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s");
        let rx = fake_daemon(
            socket.clone(),
            vec![vec![Frame::Out { b: String::new() }]],
            Duration::ZERO,
        );
        assert_eq!(
            submit_local(&socket, &request(), &waits(Duration::from_millis(600))),
            3
        );
        assert_eq!(rx.recv().unwrap(), request());
    }

    fn touch(marker: &Path) -> Request {
        Request {
            argv: vec!["touch".into(), marker.to_string_lossy().into_owned()],
            ..Request::default()
        }
    }

    #[test]
    fn with_no_daemon_the_command_does_not_run_and_the_call_fails() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let w = Waits {
            absent: Duration::from_millis(300),
            ..waits(Duration::from_secs(10))
        };
        let t0 = Instant::now();
        assert_eq!(
            submit_local(&dir.path().join("none.sock"), &touch(&marker), &w),
            NOT_RUN
        );
        assert!(t0.elapsed() >= Duration::from_millis(300), "it waited");
        assert!(!marker.exists(), "the command ran outside the pool");
    }

    #[test]
    fn with_the_opt_in_and_no_daemon_the_command_runs_here() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let w = Waits {
            fallback: true,
            ..waits(Duration::from_secs(10))
        };
        assert_eq!(
            submit_local(&dir.path().join("none.sock"), &touch(&marker), &w),
            0
        );
        assert!(marker.exists());
    }

    #[test]
    fn a_daemon_that_comes_back_within_the_wait_runs_the_job() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s");
        let (tx, rx) = mpsc::channel();
        let s = socket.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(700));
            let frames = vec![
                Frame::Start {
                    id: 1,
                    target_dir: String::new(),
                },
                Frame::Exit { code: 5 },
            ];
            tx.send(fake_daemon(s, vec![frames], Duration::ZERO))
                .unwrap();
        });
        let w = Waits {
            absent: Duration::from_secs(10),
            ..waits(Duration::from_secs(10))
        };
        assert_eq!(submit_local(&socket, &request(), &w), 5);
        assert_eq!(rx.recv().unwrap().recv().unwrap(), request());
    }

    #[test]
    fn a_job_whose_daemon_does_not_come_back_from_an_upgrade_does_not_run() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s");
        let marker = dir.path().join("ran");
        let queued = Frame::Info {
            msg: "slot: job 1 queued".into(),
        };
        let _rx = fake_daemon(socket.clone(), vec![vec![queued]], Duration::ZERO);
        assert_eq!(
            submit_local(&socket, &touch(&marker), &waits(Duration::from_millis(400))),
            NOT_RUN
        );
        assert!(!marker.exists(), "the command ran outside the pool");
    }

    #[test]
    fn a_local_request_carries_the_callers_open_file_limit() {
        let mut req = request();
        assert_eq!(req.nofile, None);
        local_fields(&mut req, Path::new("/w/sub"));
        assert_eq!(req.cwd, "/w/sub");
        assert_eq!(req.nofile, Some(crate::proto::FileLimit::of_this_process()));
        let line = serde_json::to_string(&req).unwrap();
        assert!(line.contains(r#""nofile":{"soft":"#), "{line}");
    }
}
