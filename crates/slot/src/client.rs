//! The client side of `slot <command…>`.
//!
//! Local mode talks to the daemon on this machine through its unix socket.
//! Remote mode (`SLOT_HOST` set) rsyncs the repository to the server and
//! talks to the daemon through `ssh $SLOT_HOST slot proxy`. When no daemon
//! answers, the call waits for one and then exits without running the
//! command, unless `SLOT_LOCAL_FALLBACK` asks it to run here. A job sent to
//! a server whose daemon does not answer is sent again until it does, for
//! up to `SLOT_NODE_WAIT` ([`NodeWait`]).

use std::collections::HashSet;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, Once};
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
// Each flag is a switch of its own that the client reads alone.
#[allow(clippy::struct_excessive_bools)]
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
    /// `slot warm`, or `slot run --warm` from a daemon that hands a warm-up
    /// to a node: the daemon answers at once and runs the job without the
    /// client, at its lowest level ([`Request::warm`]).
    pub warm: bool,
    /// `SLOT_NO_PRODUCTS` set and not `0`: a remote cargo job's products
    /// do not come back ([`artifacts::NO_PRODUCTS_ENV`]).
    pub no_products: bool,
    /// `SLOT_NODE_WAIT`: how long a job waits for a server whose daemon
    /// does not answer, and how often it is sent again. No wait for a job
    /// this machine's daemon handed to a node (`SLOT_SPILLED`).
    pub node_wait: NodeWait,
}

/// Names how long a job sent to a server waits for the server's daemon.
pub const NODE_WAIT_ENV: &str = "SLOT_NODE_WAIT";

/// The wait without [`NODE_WAIT_ENV`]: longer than the longest time a
/// daemon that was told to stop lets its jobs run (30 minutes).
pub const DEFAULT_NODE_WAIT: Duration = Duration::from_secs(35 * 60);

/// How often a job is sent again to a server whose daemon does not answer.
const NODE_POLL: Duration = Duration::from_secs(5);

/// What a job sent to a server does while the server's daemon does not
/// answer: a daemon that was told to stop removes its socket and lets its
/// jobs end before it starts again, and a reloading one drops the jobs it
/// has not started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeWait {
    /// How long the job is sent again before the call ends as a remote
    /// build that did not start; zero = the first lost connection ends
    /// the call.
    pub bound: Duration,
    /// The time between two sends.
    pub poll: Duration,
}

impl Default for NodeWait {
    fn default() -> Self {
        Self {
            bound: DEFAULT_NODE_WAIT,
            poll: NODE_POLL,
        }
    }
}

/// A duration from variable `name`, or `default` with a line on stderr when
/// it does not parse.
fn duration_var(name: &str, default: Duration) -> Duration {
    duration_of(name, std::env::var(name).ok().as_deref(), default)
}

/// [`duration_var`] of the variable's value.
fn duration_of(name: &str, value: Option<&str>, default: Duration) -> Duration {
    match value.filter(|v| !v.is_empty()) {
        None => default,
        Some(v) => crate::run_limit::parse_duration(v).map_or_else(
            |e| {
                eprintln!("slot: {name}: {e}; using {} s", default.as_secs());
                default
            },
            Duration::from_secs,
        ),
    }
}

/// The bound of [`NodeWait`] from the value of [`NODE_WAIT_ENV`]: a
/// duration as the other settings take it, and `0` for no wait. A job this
/// machine's daemon handed to a node (`spilled`,
/// [`crate::server::spill::SPILLED_ENV`]) does not wait, whatever the
/// value: its client ends with [`crate::server::spill::INFRA_EXIT`] and the
/// daemon queues the job here again, and a wait would count as the job's
/// run time here.
fn node_wait_of(value: Option<&str>, spilled: bool) -> Duration {
    if spilled {
        return Duration::ZERO;
    }
    match value {
        Some("0" | "0s" | "0m" | "0h") => Duration::ZERO,
        v => duration_of(NODE_WAIT_ENV, v, DEFAULT_NODE_WAIT),
    }
}

impl Settings {
    /// Reads `SLOT_HOST`, `SLOT_SSH`, `SLOT_WSROOT`, `SLOT_REMOTE`,
    /// `SLOT_ENV`, `SLOT_SOCKET`, `SLOT_LOCAL_FALLBACK`, `SLOT_SYNC_STALL`,
    /// `SLOT_SYNC_LIMIT`, `SLOT_REMOTE_SAME_PATH`, `SLOT_NO_PRODUCTS` and
    /// `SLOT_NODE_WAIT`.
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
            warm: false,
            no_products: var(artifacts::NO_PRODUCTS_ENV).is_some_and(|v| v != "0"),
            sync: sync::Bounds {
                stall: duration_var("SLOT_SYNC_STALL", sync::DEFAULT_STALL),
                limit: duration_var("SLOT_SYNC_LIMIT", sync::DEFAULT_LIMIT),
            },
            node_wait: NodeWait {
                bound: node_wait_of(
                    var(NODE_WAIT_ENV).as_deref(),
                    var(crate::server::spill::SPILLED_ENV).is_some(),
                ),
                poll: NODE_POLL,
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
    // keeps the daemon from handing the job to a node. So does a git
    // dependency of the checkout on a deny list.
    let never = never_sync::of_job(&never_sync::Job {
        cwd: &cwd,
        top: top.as_deref(),
        git_dirs: &git_dirs,
        argv,
    })
    .or_else(|| crate::git_deps::of_job(top.as_deref(), &cwd));
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
        let wanted = if s.warm {
            // A warm-up builds for the node: nothing comes back.
            None
        } else {
            artifacts::plan(argv)
        };
        // With `SLOT_NO_PRODUCTS` the command runs as written, and the plan
        // only says when to print that nothing came back.
        let (plan, skipped) = if s.no_products {
            (None, wanted)
        } else {
            (wanted, None)
        };
        if let Some(p) = &plan {
            req.argv.clone_from(&p.argv);
        }
        req.fetch_once = crate::cargo_fetch::applies(argv);
        // A same-path job outside any git checkout mirrors its working
        // directory as the tree.
        let top = top.or_else(|| keeps_path(s, &cwd).then(|| cwd.clone()));
        // The tree is synced once; the request is sent until the server's
        // daemon answers, each time with a collector of its own.
        let mut collector = None;
        let sent = remote(host, top.as_deref(), &cwd, &mut req, s).and_then(|()| {
            send_waiting(
                host,
                s.node_wait,
                |again| proxy(host, s, again),
                |c| {
                    collector = plan.as_ref().map(|p| artifacts::Collector::new(p.strip));
                    talk_with(c, &req, collector.as_mut())
                },
                &mut std::io::stderr(),
            )
        });
        let why = match sent {
            Ok(Ended::Fallback(msg)) => msg,
            Ok(ended) => {
                let ran = matches!(ended, Ended::Exit(_));
                let mut code = ended.code();
                let back = Instant::now();
                let mut came_back = false;
                if let (Some(p), Some(col), Some(top)) = (&plan, &collector, top.as_deref())
                    && ran
                    && (code == 0 || p.when == artifacts::When::AnyExit)
                {
                    came_back = true;
                    code = products_back(host, top, &cwd, s, col, code);
                }
                if let Some(p) = &skipped
                    && ran
                    && (code == 0 || p.when == artifacts::When::AnyExit)
                {
                    eprintln!(
                        "slot: artifacts not fetched: {} is set",
                        artifacts::NO_PRODUCTS_ENV
                    );
                }
                code = match (&req.workspace, top.as_deref()) {
                    (Some(ws), Some(top)) if code == 0 && !s.fetch.is_empty() => {
                        came_back = true;
                        fetch(host, top, &ws.id, s)
                    }
                    _ => code,
                };
                let back_ms = came_back.then(|| millis(back.elapsed()));
                record_phases(host, s, &req, argv, back_ms);
                return code;
            }
            Err(e) => format!("{e:#}"),
        };
        // A job the daemon handed to a node goes back to the daemon's queue.
        if std::env::var_os(crate::server::spill::SPILLED_ENV).is_some_and(|v| !v.is_empty()) {
            eprintln!("slot: remote build on {host} did not start ({why})");
            return crate::server::spill::INFRA_EXIT;
        }
        // A job sent to a server runs here only when the caller asks for it.
        if !local_fallback() {
            eprintln!(
                "slot: remote build on {host} did not start ({why}); the job did not run (set {FALLBACK_ENV}=local to run it on this machine's daemon instead)"
            );
            return crate::server::spill::INFRA_EXIT;
        }
        eprintln!(
            "slot: remote build on {host} did not start ({why}); running it on this machine's daemon"
        );
        req.argv = argv.to_vec();
        req.workspace = None;
        req.fetch_once = false;
        req.sync_ms = None;
        req.sync_bytes = None;
        req.local_reason = sync_failed(&why);
        local_fields(&mut req, &cwd);
        return submit_local(&s.socket, &req, &Waits::of(s));
    }
    local_fields(&mut req, &cwd);
    submit_local(&s.socket, &req, &Waits::of(s))
}

/// Names the one value, `local`, that lets a remote job that cannot start
/// run on this machine's daemon instead.
pub const FALLBACK_ENV: &str = "SLOT_REMOTE_FALLBACK";

/// Whether `SLOT_REMOTE_FALLBACK=local` is set.
fn local_fallback() -> bool {
    std::env::var(FALLBACK_ENV).is_ok_and(|v| v.trim() == "local")
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
        warm: s.warm,
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
/// `sync failed: ` and the first line of the error; a git dependency's
/// denial as it is.
pub fn sync_failed(why: &str) -> String {
    let first = crate::server::spill::first_line(why);
    if first.starts_with(crate::git_deps::REASON_PREFIX) {
        first
    } else if first.is_empty() {
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

/// The job's id on the server, from its `start` frame; 0 before it.
static JOB_ID: AtomicI64 = AtomicI64::new(0);

/// What the job's target directory on the server held when it queued, from
/// its `start` frame.
static JOB_SEED: Mutex<Option<String>> = Mutex::new(None);

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

/// Same-path mode is on and `top` lies under the same-path root
/// (`SLOT_REMOTE_SAME_PATH_ROOT`, by default this home; `same_path::applies`);
/// any other tree goes to the workspace root.
fn keeps_path(s: &Settings, top: &Path) -> bool {
    s.same_path && same_path::applies(top, same_path_root().as_deref())
}

/// `SLOT_REMOTE_SAME_PATH_ROOT`, by default this home.
fn same_path_root() -> Option<PathBuf> {
    std::env::var_os("SLOT_REMOTE_SAME_PATH_ROOT")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// Copies every `--fetch` path back from `host` into the repository at
/// `top`. Returns 0, or 1 after printing which path failed.
fn fetch(host: &str, top: &Path, id: &str, s: &Settings) -> i32 {
    let target_dir = JOB_TARGET.lock().map(|t| t.clone()).unwrap_or_default();
    let mirror = if keeps_path(s, top) {
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
/// `host` into the local target directory, removes the local files they
/// supersede ([`artifacts::prune`]) and prints one line. Returns
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
    let local = if keeps_path(s, top) {
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
        Ok(mut f) => {
            f.pruned = artifacts::prune(&local, &rels);
            eprintln!("{}", f.line());
            code
        }
        Err(e) => {
            eprintln!("slot: artifacts not fetched: {e:#}");
            if code == 0 { 1 } else { code }
        }
    }
}

/// How long the client waits for a server to take what it measured.
const AMEND_WAIT: Duration = Duration::from_secs(10);

/// Adds what this client measured of remote job `req` (command `argv`) to
/// the job's records: the time its products and fetches took to come back
/// (`back_ms`) to the record on `host`, whose request carried the sync; and
/// every phase to the file [`crate::phases::FILE_ENV`] names, which the
/// daemon that handed the job to `host` adds to its own record.
fn record_phases(host: &str, s: &Settings, req: &Request, argv: &[String], back_ms: Option<u64>) {
    let id = JOB_ID.load(Ordering::SeqCst);
    if back_ms.is_some() && id > 0 {
        amend(host, s, crate::proto::Amend { id, back_ms });
    }
    let Some(path) = std::env::var_os(crate::phases::FILE_ENV).filter(|v| !v.is_empty()) else {
        return;
    };
    let cargo = toolchain::for_argv(argv, 1).name() == "rust";
    let target = JOB_TARGET.lock().map(|t| t.clone()).unwrap_or_default();
    let report = crate::phases::Report {
        sync_ms: req.sync_ms,
        sync_bytes: req.sync_bytes,
        back_ms,
        node_target_dir: (cargo && !target.is_empty()).then_some(target),
        seeded_from: if cargo {
            JOB_SEED.lock().ok().and_then(|s| s.clone())
        } else {
            None
        },
    };
    if let Err(e) = report.write(Path::new(&path)) {
        eprintln!("slot: job phases not written: {e}");
    }
}

/// Sends `a` to the daemon on `host` and waits up to [`AMEND_WAIT`] for its
/// answer, printing nothing: a daemon that predates it answers "empty
/// command", which is no concern of the caller.
fn amend(host: &str, s: &Settings, a: crate::proto::Amend) {
    let req = Request {
        amend: Some(a),
        ..Request::default()
    };
    let Ok(Conn {
        reader,
        mut writer,
        mut closer,
        ..
    }) = proxy(host, s, false)
    else {
        return;
    };
    let Ok(mut line) = serde_json::to_vec(&req) else {
        return;
    };
    line.push(b'\n');
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if writer
            .write_all(&line)
            .and_then(|()| writer.flush())
            .is_ok()
        {
            drop(writer);
            for l in BufReader::new(reader).lines() {
                let Ok(l) = l else { break };
                if matches!(serde_json::from_str(&l), Ok(Frame::Exit { .. })) {
                    break;
                }
            }
        }
        let _ = tx.send(());
    });
    let _ = rx.recv_timeout(AMEND_WAIT);
    closer();
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

/// `slot warm`: after a merge to `main`, asks the daemon to build the tests
/// of the checkout that holds the working directory, at its lowest level,
/// and returns without waiting for the build. The command is the checkout's
/// `.slot.toml` `warm` entry, else [`repo_config::DEFAULT_WARM`]; `warm =
/// false` sends nothing. The daemon's answer says whether it started one.
pub fn warm(s: &Settings) -> i32 {
    if nested() {
        eprintln!("slot: no warm-up from inside a job");
        return 2;
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some(top) = checkout_info(&cwd).top else {
        eprintln!("slot: {} is in no git checkout: no warm-up", cwd.display());
        return 2;
    };
    let config = match RepoConfig::load(&top) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("slot: {e}");
            return 2;
        }
    };
    let Some(argv) = config.warm_argv() else {
        eprintln!("slot: {} switches the warm-up off", repo_config::FILE);
        return 0;
    };
    // From the repository root: the warm-up builds the whole workspace.
    if let Err(e) = std::env::set_current_dir(&top) {
        eprintln!("slot: {}: {e}", top.display());
        return 2;
    }
    let mut s = s.clone();
    s.warm = true;
    s.priority = Some(crate::types::Priority::Low);
    s.fetch.clear();
    run(&argv, &s)
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
    /// For a connection an ssh process carries: how that process ended,
    /// asked after the reader ended. `None` for a socket.
    status: Option<SshStatus>,
}

/// The exit status of a connection's ssh, or `None` when a signal ended
/// it or it did not end.
type SshStatus = Box<dyn FnOnce() -> Option<i32> + Send>;

/// The exit status ssh keeps for a failure of its own (no connection, no
/// login); any other is the status of the command it ran on the server.
const SSH_FAILED: i32 = 255;

/// How long an ssh whose output ended gets to exit before it is killed.
const SSH_EXIT_WAIT: Duration = Duration::from_secs(2);

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
        status: None,
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

/// Syncs the tree at `top` to `host` and makes `req` the request of a job
/// that runs in it there. The connection is the caller's ([`proxy`]).
fn remote(
    host: &str,
    top: Option<&Path>,
    cwd: &Path,
    req: &mut Request,
    s: &Settings,
) -> Result<()> {
    let top = top.ok_or_else(|| anyhow!("remote mode needs a git repository"))?;
    if let Some(why) = same_path::tree_refusal(top, same_path_root().as_deref()) {
        return Err(anyhow!(why));
    }
    let id = workspace_id(top);
    let same_path = keeps_path(s, top);
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
        same_path,
    };
    let t0 = Instant::now();
    let synced = sync::sync(top, &id, tc.sync_excludes(), &target, &s.sync)?;
    eprintln!("{}", synced.line(host));
    let target_dir = if same_path {
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
    req.sync_ms = Some(millis(t0.elapsed()));
    req.sync_bytes = synced.bytes;
    // The git dependencies the lock files name, from this machine's cargo
    // git database: the node may have no way to their sources. The lists
    // are read again: what is copied is checked as it is copied.
    let lists = crate::git_deps::lists()
        .map_err(|e| anyhow!("{}list unreadable ({e})", crate::git_deps::REASON_PREFIX))?;
    match crate::git_deps::carry(top, cwd, &req.argv, &lists, &target, &s.sync) {
        Ok(Some(line)) => eprintln!("{line}"),
        Ok(None) => {}
        Err(crate::git_deps::CarryError::Skip(e)) => {
            eprintln!("slot: git dependencies not copied to {host}: {e}");
        }
        Err(crate::git_deps::CarryError::Refuse(e)) => return Err(anyhow!("{e}")),
    }
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
        same_path,
        target_dir,
    });
    Ok(())
}

/// Whole milliseconds of `d`.
fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// A connection to the daemon on `host` through `ssh host slot proxy`.
/// With `quiet`, what ssh and the server's step write to stderr is dropped:
/// a job that is sent again says why once, not every time.
fn proxy(host: &str, s: &Settings, quiet: bool) -> Result<Conn> {
    let mut child: Child = Command::new(&s.ssh[0])
        .args(&s.ssh[1..])
        .arg(host)
        .arg(&s.remote_slot)
        .arg("proxy")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(if quiet {
            Stdio::null()
        } else {
            Stdio::inherit()
        })
        .spawn()
        .context("ssh")?;
    let writer = child.stdin.take().ok_or_else(|| anyhow!("ssh stdin"))?;
    let reader = child.stdout.take().ok_or_else(|| anyhow!("ssh stdout"))?;
    let child = Arc::new(Mutex::new(child));
    let ended = Arc::clone(&child);
    Ok(Conn {
        reader: Box::new(reader),
        writer: Box::new(writer),
        closer: Box::new(move || {
            if let Ok(mut c) = child.lock() {
                let _ = c.kill();
            }
        }),
        status: Some(Box::new(move || exit_status(&ended))),
    })
}

/// The exit status of `ssh`, whose output ended: it gets [`SSH_EXIT_WAIT`]
/// to exit and is killed after that. The lock is held for one look at a
/// time: the signal thread takes it to kill the process.
fn exit_status(ssh: &Mutex<Child>) -> Option<i32> {
    let deadline = Instant::now() + SSH_EXIT_WAIT;
    loop {
        {
            let mut c = ssh.lock().ok()?;
            match c.try_wait() {
                Ok(Some(st)) => return st.code(),
                Ok(None) if Instant::now() < deadline => {}
                _ => {
                    let _ = c.kill();
                    let _ = c.wait();
                    return None;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Sends a job to the daemon on `host`: `connect` opens a connection (its
/// argument says whether the job is being sent again), `talk` holds the
/// conversation. A conversation the daemon did not answer (the connection
/// closed before the job's process started, or the request could not be
/// written) is held again every `w.poll`, with one line on `err`, until
/// one gets past that point or `w.bound` has passed: then the error names
/// the wait, and the job did not start. Not sent again: a job whose
/// process had started, one whose client got a signal, and one whose ssh
/// failed by itself ([`SSH_FAILED`]): the server was not reached, which is
/// not a daemon that restarts.
fn send_waiting(
    host: &str,
    w: NodeWait,
    mut connect: impl FnMut(bool) -> Result<Conn>,
    mut talk: impl FnMut(Conn) -> Ended,
    err: &mut dyn Write,
) -> Result<Ended> {
    let t0 = Instant::now();
    let mut again = false;
    loop {
        let mut conn = connect(again)?;
        let status = conn.status.take();
        let ended = talk(conn);
        let unanswered = matches!(ended, Ended::Lost { started: false } | Ended::Unsent(_));
        if !unanswered || w.bound.is_zero() || signalled().is_some() {
            return Ok(ended);
        }
        if status.and_then(|ssh| ssh()) == Some(SSH_FAILED) {
            if again {
                let _ = writeln!(err, "slot: ssh to {host} failed while the job waited");
            }
            return Ok(ended);
        }
        let left = w.bound.saturating_sub(t0.elapsed());
        if left.is_zero() {
            return Err(anyhow!(
                "its daemon did not answer within {}",
                crate::run_limit::text(w.bound.as_secs())
            ));
        }
        if !again {
            let _ = writeln!(
                err,
                "slot: waiting for {host}: its daemon does not answer (a restart or a drain); the job is sent again when it answers"
            );
            let _ = err.flush();
        }
        again = true;
        // A conversation that sent nothing installed no signal thread.
        handle_signals();
        if let Some(sig) = pause(w.poll.min(left)) {
            return Ok(Ended::Signalled(sig));
        }
    }
}

/// Sleeps for `d`, or until this client gets a signal: then the signal.
fn pause(d: Duration) -> Option<i32> {
    let until = Instant::now() + d;
    loop {
        if let Some(sig) = signalled() {
            return Some(sig);
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        std::thread::sleep(left.min(Duration::from_millis(100)));
    }
}

/// How a conversation with the daemon ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ended {
    /// The job's exit code.
    Exit(i32),
    /// The request could not be written to the connection, for this
    /// reason; the daemon did not get it.
    Unsent(String),
    /// The connection closed without an exit frame. `started` is whether
    /// the job's process had started (a start, output or error frame came).
    Lost { started: bool },
    /// The client got this signal; the job was cancelled.
    Signalled(i32),
    /// The server sent the job back without starting it, for this reason.
    Fallback(String),
}

impl Ended {
    /// The exit code for the caller; a lost connection and a request that
    /// was not sent are reported here.
    fn code(self) -> i32 {
        match self {
            Self::Exit(code) => code,
            Self::Signalled(sig) => 128 + sig,
            Self::Unsent(why) => {
                eprintln!("slot: send: {why}");
                3
            }
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
        ..
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
        return Ended::Unsent(e.to_string());
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
                        write_out(&mut stdout, &c.push(b.as_bytes()));
                    }
                    None => {
                        write_out(&mut stdout, b.as_bytes());
                    }
                }
                let _ = stdout.flush();
            }
            Ok(Frame::Err { b }) => {
                started = true;
                write_out(&mut stderr, b.as_bytes());
            }
            Ok(Frame::Start {
                id,
                target_dir,
                seeded_from,
            }) => {
                started = true;
                if let Ok(mut t) = JOB_TARGET.lock() {
                    *t = target_dir;
                }
                JOB_ID.store(id, Ordering::SeqCst);
                if let Ok(mut s) = JOB_SEED.lock() {
                    *s = seeded_from;
                }
            }
            Ok(Frame::Info { msg }) => eprintln!("{msg}"),
            Ok(Frame::Exit { code }) => {
                drop(writer);
                if let Some(c) = collector.as_deref_mut() {
                    write_out(&mut stdout, &c.finish());
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
        write_out(&mut stdout, &c.finish());
        let _ = stdout.flush();
    }
    if let Some(sig) = signalled() {
        return Ended::Signalled(sig);
    }
    Ended::Lost { started }
}

/// Writes all of `buf`, also to a pipe that another process sharing it made
/// non-blocking (a test runner or bundler in the same pipeline): on
/// `WouldBlock` it waits briefly and writes the rest. `write_all` gives up
/// there, and the rest of the block was lost while later lines arrived.
fn write_fully<W: Write>(w: &mut W, mut buf: &[u8]) -> std::io::Result<()> {
    while !buf.is_empty() {
        match w.write(buf) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(n) => buf = &buf[n..],
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// [`write_fully`], then a flush that also waits out `WouldBlock`.
fn write_out<W: Write>(w: &mut W, buf: &[u8]) {
    if write_fully(w, buf).is_err() {
        return;
    }
    loop {
        match w.flush() {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            _ => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pipe another process made non-blocking: takes at most 3 bytes per
    /// call and answers every second call with `WouldBlock`.
    struct Choppy {
        got: Vec<u8>,
        calls: usize,
    }

    impl Write for Choppy {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.calls += 1;
            if self.calls.is_multiple_of(2) {
                return Err(std::io::ErrorKind::WouldBlock.into());
            }
            let n = buf.len().min(3);
            self.got.extend_from_slice(&buf[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn output_to_a_non_blocking_pipe_arrives_whole() {
        let text = b"test lease::renews ... ok\nSummary [ 1.2s] 40 tests run: 40 passed\n";
        let mut w = Choppy {
            got: Vec::new(),
            calls: 0,
        };
        write_fully(&mut w, text).unwrap();
        assert_eq!(w.got, text);
    }
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
                        seeded_from: None,
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
                    seeded_from: None,
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
                    seeded_from: None,
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

    /// What one connection of [`Node`] does with the request it gets.
    enum Answer {
        /// Takes the request, answers with these frames and closes; its
        /// ssh ends with this status.
        Frames(Vec<Frame>, Option<i32>),
        /// Takes no byte: the write fails.
        Refuses,
    }

    /// A writer that keeps what it gets, or fails.
    struct Taken(Option<Arc<Mutex<Vec<String>>>>);

    impl Write for Taken {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let Some(got) = &self.0 else {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            };
            got.lock()
                .unwrap()
                .push(String::from_utf8_lossy(buf).into_owned());
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A stand-in for a server reached through ssh: one answer per
    /// connection, in order, and the last one for every connection after.
    struct Node {
        answers: Vec<Answer>,
        /// The `again` argument of each connection.
        connects: Vec<bool>,
        /// What each connection got written.
        got: Arc<Mutex<Vec<String>>>,
    }

    impl Node {
        fn new(answers: Vec<Answer>) -> Self {
            Self {
                answers,
                connects: Vec::new(),
                got: Arc::default(),
            }
        }

        fn connect(&mut self, again: bool) -> Conn {
            let n = self.connects.len().min(self.answers.len() - 1);
            self.connects.push(again);
            let (text, writer, status) = match &self.answers[n] {
                Answer::Frames(frames, status) => {
                    let mut text = Vec::new();
                    for f in frames {
                        text.extend(serde_json::to_vec(f).unwrap());
                        text.push(b'\n');
                    }
                    (text, Taken(Some(self.got.clone())), *status)
                }
                Answer::Refuses => (Vec::new(), Taken(None), Some(1)),
            };
            Conn {
                reader: Box::new(std::io::Cursor::new(text)),
                writer: Box::new(writer),
                closer: Box::new(|| {}),
                status: Some(Box::new(move || status)),
            }
        }

        /// The requests the connections got, one line each.
        fn requests(&self) -> Vec<Request> {
            self.got
                .lock()
                .unwrap()
                .iter()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect()
        }
    }

    /// Sends [`request`] to `node` as a job of host `node-1`; returns how
    /// it ended and what the caller read on stderr.
    fn sent_to(node: &mut Node, w: NodeWait) -> (Result<Ended, String>, String) {
        let mut err = Vec::new();
        let req = request();
        let ended = send_waiting(
            "node-1",
            w,
            |again| Ok(node.connect(again)),
            |c| talk(c, &req),
            &mut err,
        );
        (
            ended.map_err(|e| format!("{e:#}")),
            String::from_utf8(err).unwrap(),
        )
    }

    /// A wait of `bound_ms` with a send every 20 ms.
    fn node_wait(bound_ms: u64) -> NodeWait {
        NodeWait {
            bound: Duration::from_millis(bound_ms),
            poll: Duration::from_millis(20),
        }
    }

    const WAITING: &str = "slot: waiting for node-1: its daemon does not answer (a restart or a drain); the job is sent again when it answers\n";

    fn started() -> Frame {
        Frame::Start {
            id: 1,
            target_dir: String::new(),
            seeded_from: None,
        }
    }

    #[test]
    fn a_job_whose_nodes_daemon_does_not_answer_is_sent_again_until_it_does() {
        let queued = Frame::Info {
            msg: "slot: job 1 queued".into(),
        };
        let mut node = Node::new(vec![
            // The server's step found no socket: a daemon that drains.
            Answer::Frames(vec![], Some(3)),
            // ssh was gone before the request was written.
            Answer::Refuses,
            // A reloading daemon dropped the job it had queued.
            Answer::Frames(vec![queued], Some(0)),
            Answer::Frames(vec![started(), Frame::Exit { code: 7 }], Some(0)),
        ]);
        let t0 = Instant::now();
        let (ended, err) = sent_to(&mut node, node_wait(10_000));
        assert_eq!(ended, Ok(Ended::Exit(7)), "{err}");
        assert_eq!(err, WAITING, "one line, however often the job is sent");
        assert_eq!(
            node.connects,
            [false, true, true, true],
            "every connection after the first says the job is sent again"
        );
        assert_eq!(
            node.requests(),
            [request(), request(), request()],
            "the same request each time"
        );
        assert!(
            t0.elapsed() >= Duration::from_millis(60),
            "a poll between two sends"
        );
    }

    #[test]
    fn at_the_bound_a_job_whose_nodes_daemon_does_not_answer_did_not_start() {
        let mut node = Node::new(vec![Answer::Frames(vec![], Some(3))]);
        let t0 = Instant::now();
        let (ended, err) = sent_to(&mut node, node_wait(90));
        let why = ended.unwrap_err();
        assert!(
            why.starts_with("its daemon did not answer within "),
            "{why}"
        );
        assert_eq!(err, WAITING);
        assert!(t0.elapsed() >= Duration::from_millis(90));
        assert!(
            node.connects.len() >= 2 && node.connects.len() <= 6,
            "sent every poll up to the bound: {:?}",
            node.connects
        );
        let secs = NodeWait {
            bound: Duration::from_secs(1),
            poll: Duration::from_secs(1),
        };
        let (ended, _) = sent_to(&mut node, secs);
        assert_eq!(
            ended,
            Err("its daemon did not answer within 1s".into()),
            "the bound as the settings write it"
        );
    }

    #[test]
    fn a_job_whose_ssh_failed_is_not_sent_again() {
        let mut node = Node::new(vec![Answer::Frames(vec![], Some(SSH_FAILED))]);
        let (ended, err) = sent_to(&mut node, node_wait(10_000));
        assert_eq!(ended, Ok(Ended::Lost { started: false }));
        assert_eq!(err, "", "no wait for a server that was not reached");
        assert_eq!(node.connects, [false]);
    }

    #[test]
    fn a_job_that_started_on_a_node_is_not_sent_again() {
        let mut node = Node::new(vec![Answer::Frames(vec![started()], Some(0))]);
        let (ended, err) = sent_to(&mut node, node_wait(10_000));
        assert_eq!(ended, Ok(Ended::Lost { started: true }));
        assert_eq!(err, "");
        assert_eq!(node.connects, [false], "its process may have run");
    }

    #[test]
    fn with_a_node_wait_of_zero_a_lost_job_ends_at_once() {
        let mut node = Node::new(vec![Answer::Frames(vec![], Some(3))]);
        let (ended, err) = sent_to(&mut node, node_wait(0));
        assert_eq!(ended, Ok(Ended::Lost { started: false }));
        assert_eq!(err, "");
        assert_eq!(node.connects, [false]);

        let mut node = Node::new(vec![Answer::Refuses]);
        let (ended, _) = sent_to(&mut node, node_wait(0));
        assert_eq!(ended, Ok(Ended::Unsent("broken pipe".into())));
        assert_eq!(node.connects, [false]);
    }

    #[test]
    fn the_node_wait_is_35_minutes_unless_the_variable_names_another() {
        assert_eq!(node_wait_of(None, false), Duration::from_secs(35 * 60));
        assert_eq!(Settings::default().node_wait, NodeWait::default());
        assert_eq!(NodeWait::default().bound, DEFAULT_NODE_WAIT);
        assert_eq!(NodeWait::default().poll, Duration::from_secs(5));
        assert_eq!(node_wait_of(Some("90"), false), Duration::from_secs(90));
        assert_eq!(node_wait_of(Some("10m"), false), Duration::from_secs(600));
        for zero in ["0", "0s", "0m", "0h"] {
            assert_eq!(node_wait_of(Some(zero), false), Duration::ZERO, "{zero}");
        }
        assert_eq!(
            node_wait_of(Some("soon"), false),
            DEFAULT_NODE_WAIT,
            "a value that is no duration"
        );
    }

    /// A job this machine's daemon handed to a node has that daemon's own
    /// rule: its client ends with the exit code that puts the job back in
    /// the daemon's queue, at once, whatever the variable names.
    #[test]
    fn a_job_a_daemon_handed_to_a_node_does_not_wait_for_the_nodes_daemon() {
        assert_eq!(node_wait_of(None, true), Duration::ZERO);
        let w = NodeWait {
            bound: node_wait_of(Some("30s"), true),
            poll: Duration::from_millis(20),
        };
        let mut node = Node::new(vec![
            Answer::Frames(vec![], Some(3)),
            Answer::Frames(vec![started(), Frame::Exit { code: 0 }], Some(0)),
        ]);
        let (ended, err) = sent_to(&mut node, w);
        assert_eq!(err, "", "no wait");
        assert_eq!(node.connects, [false], "not sent again");
        assert_eq!(ended, Ok(Ended::Lost { started: false }));
        assert_eq!(
            ended.map(Ended::code),
            Ok(crate::server::spill::INFRA_EXIT),
            "the daemon queues it here again"
        );
    }
}
