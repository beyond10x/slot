//! `slot serve`: queues jobs on the jobserver pool, runs them, streams output
//! to the client and publishes live state to the web UI.

pub mod cache;
pub mod handover;
mod http;
pub mod limits;
pub mod machine;
mod metrics;
pub mod nodes;
pub mod priority;
mod ring;
pub mod spill;
mod tally;
mod upgrade;

use tally::Tally;

use spill::Ran;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::SocketAddr;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
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
use crate::cpu_limit;
use crate::job_parallel;
use crate::jobserver::{self, Pool, WaiterHandle};
use crate::policy;
use crate::procstat::{self, Proc, Snapshot, Tracker};
use crate::proto::{Frame, Request};
use crate::rewrite::Rewriter;
use crate::run_limit;
use crate::toolchain::{self, Context};
use crate::types::{
    Event, HeadroomBase, HostCensus, Job, JobState, LOCAL_NODE, LiveJob, Node, NodeKind, NodeState,
    Priority, SccacheWatch, State, Unit,
};

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
    /// Watch sccache servers: give back jobserver tokens they lost, and
    /// log their counters once a minute.
    pub sccache_watch: bool,
    /// Free memory and disk a job needs before it may start; 0 turns one off.
    pub admission: admission::Limits,
    /// Longest time an upgrade stops admission while jobs run; zero never stops it.
    pub upgrade_hold: Duration,
    /// A queued job that waited longer than this is ordered as `high`;
    /// zero never promotes. A delay saved through `PUT /api/priority` wins.
    pub promote_after: Duration,
    /// A queued job may be passed by later jobs (one predicted to run a few
    /// seconds, one that fits in slots a larger request collected) only
    /// until it has waited this long; zero: nobody passes, the queue is
    /// served by level, then arrival.
    pub pass_for: Duration,
    /// The clock a queued job's wait is measured by for that, ms since the
    /// epoch: [`now_ms`] outside tests.
    pub clock: fn() -> i64,
    /// How often a job that waits for its target directory looks at it
    /// again: [`TARGET_POLL`] outside tests.
    pub target_poll: Duration,
    /// How long after its start a job counts as about to take cargo's lock
    /// on its target directory: [`TARGET_SETTLE`] outside tests.
    pub target_settle: Duration,
    /// How often the token watch looks for lost and surplus jobserver
    /// tokens: [`TOKEN_WATCH`] outside tests.
    pub token_watch: Duration,
    /// How often a job held for disk, memory or the host's CPUs looks
    /// again: [`RESOURCE_POLL`] outside tests.
    pub resource_poll: Duration,
    /// The host's busy CPUs, in parts, for the CPU limit (`slot
    /// cpu-limit`). `None` reads the census sampler's figure, as everywhere
    /// outside tests.
    pub cpu_busy: Option<fn() -> Option<cpu_limit::Parts>>,
    /// CPUs of the host; `None` counts them, as everywhere outside tests.
    pub host_cpus: Option<u32>,
    /// The longest a job is held for the host's CPUs:
    /// [`cpu_limit::HOLD_MAX`] outside tests.
    pub cpu_hold_max: Duration,
    /// After a job started on that bound, how long the next job held as
    /// long must wait: [`cpu_limit::STARTING_MS`] outside tests, so held
    /// jobs that reach the bound together start one by one.
    pub cpu_bound_gap: Duration,
    /// CPU pressure of the host (`some avg10`, percent), for the brake of
    /// admission by share. `None` reads the census sampler's, as everywhere
    /// outside tests.
    pub cpu_pressure: Option<fn() -> f64>,
    /// Bytes the target directories of synced workspaces may take together
    /// before the least recently used are deleted; 0 = no budget (the disk
    /// floor still evicts).
    pub cache_budget: u64,
    /// Eviction of local target directories under a disk floor.
    pub evict: crate::evict::Settings,
    /// Free bytes on the filesystem of a path, for the disk hold and
    /// eviction. `None` asks the filesystem, as everywhere outside tests.
    pub disk_free: Option<fn(&Path) -> Option<u64>>,
    /// The program that understands `slot __exec`, for a job whose
    /// open-file limit is not the daemon's: `/proc/self/exe` (the daemon's
    /// own image, also once the file was replaced) outside tests. `None`
    /// runs every job with the daemon's limit.
    pub exec_shim: Option<PathBuf>,
    /// On reload, hand running and queued jobs to the new binary instead of
    /// waiting for an idle pool (`handover`); the bounded hold is the fallback.
    pub handover: bool,
    /// Run every job in this sandbox (`slot serve --sandbox`); `None` runs
    /// jobs as the daemon's user with its whole filesystem.
    pub sandbox: Option<crate::sandbox::Sandbox>,
    /// The host configuration file (`cluster.toml`) whose remote nodes the
    /// daemon polls; `None` polls none.
    pub cluster_file: Option<PathBuf>,
    /// Directories under which a remote client may mirror its tree at its
    /// own absolute path (`--same-path-root`); empty refuses such jobs.
    pub same_path_roots: Vec<PathBuf>,
}

/// systemd slice the scopes of jobs under `--memory-max` run in; the
/// headroom's CPU quota is set on it too.
pub const JOB_SLICE: &str = "slot.slice";

/// How often a job that waits for disk or memory looks again.
pub const RESOURCE_POLL: Duration = Duration::from_secs(2);

/// How often a job that waits for its cargo target directory looks at the
/// directory's lock again, at the gate and in the queue for slots.
pub const TARGET_POLL: Duration = Duration::from_secs(1);

/// A cargo job that started this recently counts as about to compile: cargo
/// reads its configuration and resolves the workspace before it takes the
/// lock on the build directory, so the lock is not held yet.
pub const TARGET_SETTLE: Duration = Duration::from_secs(3);

/// Jobs that started this recently count as still growing towards their
/// expected peak memory.
const STARTING_MS: i64 = 30_000;

/// Interval between two call probes.
const CALL_PROBE: Duration = Duration::from_secs(5);

/// Interval between two readings of the sccache counters.
const CACHE_STATS_EVERY: Duration = Duration::from_secs(60);

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

/// How often [`Server::token_watch`] looks for lost and surplus jobserver
/// tokens.
pub const TOKEN_WATCH: Duration = Duration::from_secs(15);

/// What the token watch keeps from its last pass.
#[derive(Debug, Default)]
struct TokenMemo {
    /// The shortfall of the last pass, when it saw one and wrote nothing back.
    short: Option<usize>,
    /// The surplus of the last pass, when it saw one and took nothing out.
    surplus: Option<usize>,
}

// Each flag is a fact of its own that the queue and the sampler read alone.
#[allow(clippy::struct_excessive_bools)]
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
    /// CPUs the job is expected to use while it runs, from earlier runs.
    expect_cores: f64,
    /// The job's CPUs over the last seconds, as the sampler saw them.
    recent: cpu_limit::Recent,
    /// Queued and weighed against the CPU limit: held for the host's CPUs
    /// ([`Server::resources_ready`]), or let through after such a hold and
    /// not started yet. The held jobs behind it count it, so they start in
    /// the queue's order.
    cpu_wait: bool,
    /// When the job was first held for the host's CPUs, Unix ms.
    cpu_wait_since: Option<i64>,
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
    /// Queued and at the gate of its cargo target directory
    /// ([`Server::target_ready`]): from its first look until it stands in
    /// the queue for slots.
    target_wait: bool,
    /// Set to `true` when the job ran past its run-time limit
    /// ([`Server::end_overdue`]); [`Server::supervise`] then ends it.
    overdue: watch::Sender<bool>,
    /// The share of the CPUs it asks for by share ([`crate::shares`]).
    share: f64,
    /// It claimed its start: with admission by share it holds no slot, so
    /// `held` does not tell a job that is about to start from a queued one.
    claimed: bool,
    /// It may go to a remote node ([`spill`]) and has not been there.
    spillable: bool,
    /// The target directory its environment names (`CARGO_TARGET_DIR`,
    /// [`crate::evict::env_target`]), whatever its toolchain: eviction
    /// keeps it.
    env_target: Option<PathBuf>,
}

impl Live {
    /// Whether the job runs, or claimed its start and has no process yet.
    fn started(&self) -> bool {
        self.pid.is_some() || self.held > 0 || self.claimed
    }
}

/// What stands between a queued cargo job and its target directory: the
/// other jobs that compile into the same one.
#[derive(Debug, Default, PartialEq, Eq)]
struct TargetUse {
    /// The target directory.
    dir: PathBuf,
    /// A job that holds slots and has no process yet, or started less than
    /// the settle time ago: about to take cargo's lock.
    starting: Option<i64>,
    /// A job that has run for longer: it holds the lock while it compiles.
    running: Option<i64>,
    /// A queued job that is served first (higher level, else older) and
    /// waits for the directory too or stands in the queue for slots.
    ahead: Option<i64>,
}

/// Whether `l` is a local cargo job that compiles, and the target directory
/// it compiles into.
fn build_dir(l: &Live) -> Option<&str> {
    (!l.job.target_dir.is_empty() && crate::cargo_lock::compiles(&l.job.argv))
        .then_some(l.job.target_dir.as_str())
}

/// The jobs in `active` that compile into the target directory of job `id`;
/// `None` when `id` is not a local cargo job that compiles. Of several jobs
/// of one kind it names the one with the lowest id (`ahead`: the one served
/// first).
fn target_use(active: &BTreeMap<i64, Live>, id: i64, now: i64, settle: i64) -> Option<TargetUse> {
    let me = active.get(&id)?;
    let dir = build_dir(me)?;
    let mut u = TargetUse {
        dir: PathBuf::from(dir),
        ..TargetUse::default()
    };
    let mut ahead = (me.job.priority, id);
    for (other, l) in active {
        if *other == id || build_dir(l) != Some(dir) {
            continue;
        }
        if l.pid.is_some() {
            let slot = if now - l.job.started_ms < settle {
                &mut u.starting
            } else {
                &mut u.running
            };
            slot.get_or_insert(*other);
        } else if l.started() {
            u.starting.get_or_insert(*other);
        } else if (l.target_wait || l.waiter.is_some()) && (l.job.priority, *other) < ahead {
            ahead = (l.job.priority, *other);
            u.ahead = Some(*other);
        }
    }
    Some(u)
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
    /// Jobs this node ended over the last 25 hours, from the whole job log.
    tally: Tally,
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
    /// The host's busy CPUs over the last seconds, in parts, for the CPU
    /// limit.
    host_busy: cpu_limit::RecentParts,
    /// When a job last started on the CPU hold's bound, Unix ms.
    cpu_bound_ms: i64,
    controller: policy::Controller,
    /// Peak memory of finished jobs, for admission.
    memory: admission::History,
    /// Set once a handover took the job table: nothing finishes, reaps,
    /// starts or answers a client after it in this process.
    handing_over: bool,
    /// Local target directories jobs built into: repository and last build,
    /// Unix milliseconds. Eviction only ever deletes one of these.
    targets: HashMap<PathBuf, (String, i64)>,
    /// Working directories of the jobs of the log and when a job last ran
    /// in each, Unix ms: eviction keeps the target directories in the tree
    /// of one used in the last hour ([`crate::evict::TREE_KEEP`]).
    ran_in: HashMap<PathBuf, i64>,
    /// Machine base: the cgroup readings and their averages.
    machine: machine::Monitor,
    /// Machine base: the numbers of the last tick.
    machine_shown: machine::Shown,
    /// Admission by share: the CPUs the running jobs may use together.
    budget: crate::shares::Budget,
    /// Admission by share: the figures of the last look.
    share_view: crate::shares::View,
    /// Admission by share: when it last looked, Unix ms.
    share_last_ms: Option<i64>,
    /// CPU pressure of the host at the last census sample, percent.
    psi_cpu: f64,
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
    /// Passes of the token watch since start.
    watch_passes: AtomicU64,
    /// Surplus jobserver tokens taken out since start.
    removed: AtomicU64,
    /// Tokens that neither a job nor a live holder of the FIFO accounts
    /// for, as the token watch saw them last, and since when (Unix
    /// milliseconds; 0 while there are none).
    unaccounted: AtomicU64,
    unaccounted_since: AtomicI64,
    /// Remote nodes of the host configuration.
    remotes: nodes::Remotes,
    /// What eviction of target directories did last.
    evict: Mutex<crate::evict::Report>,
    /// Target directories being deleted now: their jobs wait.
    evicting: Mutex<HashSet<PathBuf>>,
    /// Woken by a job held for disk, so eviction looks at once
    /// ([`Server::evict_watch`]).
    evict_wake: tokio::sync::Notify,
    /// Woken by `slot evict-below`, so eviction looks at once at the new
    /// mark.
    evict_set: tokio::sync::Notify,
    /// The eviction mark set with `slot evict-below`, GB, as saved in
    /// `limits.json`; `None` = `--evict-below`.
    evict_below: Mutex<Option<f64>>,
    /// The idle keep set with `slot evict-below --idle`, seconds, as saved
    /// in `limits.json`; `None` = [`crate::evict::Settings::min_idle`] of
    /// the config.
    evict_idle: Mutex<Option<u64>>,
    /// Target directories whose removal failed, with the time (Unix
    /// milliseconds): eviction leaves them out for
    /// [`crate::evict::FAILED_KEEP`], so a pass plans the next ones.
    evict_failed: Mutex<HashMap<PathBuf, i64>>,
    /// The daemon's cgroup v2 path, from `/proc/self/cgroup`.
    own_cgroup: Option<String>,
    /// The daemon's user id, which names its login sessions' cgroups.
    uid: u32,
    /// Held while a `CPUQuota` is set, so two settings never cross.
    quota_lock: tokio::sync::Mutex<()>,
    /// The priority rules and the promotion delay set at run time, as
    /// saved in `priority.json`.
    priority: Mutex<priority::Table>,
    /// The run-time limit and the most a job may name, as saved in
    /// `limits.json`.
    run_limit: Mutex<run_limit::Limits>,
    /// Build processes one job may run at once (`slot job-parallel`), as
    /// saved in `limits.json`; `None` = no limit, jobs get the pool's
    /// jobserver.
    job_parallel: Mutex<Option<usize>>,
    /// The mark and the floor of the CPU limit (`slot cpu-limit`), as saved
    /// in `limits.json`.
    cpu_limit: Mutex<cpu_limit::Limits>,
    /// How queued jobs are admitted, with the bounds and the pressure mark
    /// of admission by share (`slot admission`), as saved in `limits.json`.
    shares: Mutex<crate::shares::Settings>,
    /// When this process started, Unix ms.
    born_ms: i64,
}

/// Root of the cgroup v2 hierarchy.
const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// The headroom in force, whether its CPU quota is applied, and call mode.
#[derive(Debug, Clone, Default)]
struct Headroom {
    percent: u8,
    quota: Option<u32>,
    note: String,
    /// What the headroom is a share of.
    base: HeadroomBase,
    /// Machine base: the quota last set, CPUs, and when.
    machine_applied: Option<(f64, Instant)>,
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

impl Headroom {
    /// Records the outcome of setting `quota`.
    fn record_quota(&mut self, quota: Option<u32>, applied: Result<(), String>) {
        match applied {
            Ok(()) => {
                self.quota = quota;
                self.note = String::new();
            }
            Err(e) => {
                self.quota = None;
                self.note = e;
            }
        }
    }
}

/// Milliseconds since the epoch.
pub fn now_ms() -> i64 {
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
        let priority_table = priority::load(&cfg.state_dir);
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
        let mut ran_in = HashMap::new();
        let mut tally = Tally::default();
        let started = (cfg.clock)();
        if let Ok(f) = File::open(&log_path) {
            for line in BufReader::new(f).lines().map_while(Result::ok) {
                if let Ok(job) = serde_json::from_str::<Job>(&line) {
                    next_id = next_id.max(job.id);
                    memory.record(&job);
                    record_target(&mut targets, &job);
                    record_ran(&mut ran_in, &job);
                    tally.record(&job, started);
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
        let mut history = VecDeque::new();
        if let Some(h) = &mut inherited {
            next_id = next_id.max(h.state.next_id);
            history = std::mem::take(&mut h.state.history);
            history.drain(..history.len().saturating_sub(HISTORY_TICKS));
            for d in done.iter_mut().rev().take(DONE_OUTPUT_KEEP) {
                if let Some(tail) = h.state.done_output.remove(&d.job.id) {
                    let ring = Arc::new(Ring::from_tail(OUTPUT_KEEP, tail));
                    ring.close();
                    d.out = Some(ring);
                }
            }
            h.state.done_output.clear();
        }
        let cores = cfg
            .host_cpus
            .unwrap_or_else(|| procstat::host_cpus() as u32);
        let pool_size = cfg.tokens;
        let call_headroom = cfg.call_headroom;
        let sccache_watch = cfg.sccache_watch;
        let upgrade_hold = cfg.upgrade_hold;
        let evict_settings = cfg.evict;
        let remotes = nodes::Remotes::load(cfg.cluster_file.as_deref(), &host);
        let server = Arc::new(Self {
            cfg,
            pool,
            inner: Mutex::new(Inner {
                next_id,
                active: BTreeMap::new(),
                done,
                tally,
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
                host_busy: cpu_limit::RecentParts::default(),
                cpu_bound_ms: 0,
                controller: policy::Controller::new(policy::Settings::new(
                    cores as usize,
                    pool_size,
                )),
                memory,
                handing_over: false,
                targets,
                ran_in,
                machine: machine::Monitor::default(),
                machine_shown: machine::Shown::default(),
                budget: crate::shares::Budget::new(cores, f64::from(cores)),
                share_view: crate::shares::View::default(),
                share_last_ms: None,
                psi_cpu: 0.0,
            }),
            events,
            history: Mutex::new(history),
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
                base: saved.base,
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
            watch_passes: AtomicU64::new(0),
            removed: AtomicU64::new(0),
            unaccounted: AtomicU64::new(0),
            unaccounted_since: AtomicI64::new(0),
            remotes,
            evict: Mutex::new(crate::evict::Report {
                floor: crate::evict::below(saved.evict_below_gb, evict_settings.floor),
                dry_run: evict_settings.dry_run,
                ..crate::evict::Report::default()
            }),
            evicting: Mutex::new(HashSet::new()),
            evict_wake: tokio::sync::Notify::new(),
            evict_set: tokio::sync::Notify::new(),
            evict_below: Mutex::new(saved.evict_below_gb),
            evict_idle: Mutex::new(saved.evict_idle_s),
            evict_failed: Mutex::new(HashMap::new()),
            own_cgroup: std::fs::read_to_string("/proc/self/cgroup")
                .ok()
                .and_then(|s| machine::own_cgroup(&s)),
            uid: nix::unistd::getuid().as_raw(),
            quota_lock: tokio::sync::Mutex::new(()),
            priority: Mutex::new(priority_table),
            run_limit: Mutex::new(run_limit::Limits {
                run_limit_s: saved.run_limit_s,
                run_limit_max_s: saved.run_limit_max_s,
            }),
            job_parallel: Mutex::new(saved.job_parallel),
            cpu_limit: Mutex::new(cpu_limit::Limits::loaded(
                saved.cpu_busy_max,
                saved.cpu_busy_floor,
                saved.cpu_pressure_max,
            )),
            shares: Mutex::new(saved.shares()),
            born_ms: now_ms(),
        });
        server.apply_saved_admission();
        Ok(server)
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
            if !l.started() {
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
        g.active.values().all(|l| !l.started())
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
        tokio::spawn(Arc::clone(&self).priority_watch());
        tokio::spawn(Arc::clone(&self).run_limit_watch());
        tokio::spawn(Arc::clone(&self).node_poll());
        // Always: `slot evict-below` may turn eviction on at run time.
        tokio::spawn(Arc::clone(&self).evict_watch());
        if self.cfg.sccache_watch {
            tokio::spawn(Arc::clone(&self).sccache_watch());
            tokio::spawn(Arc::clone(&self).cache_stats_watch());
        }
        let h = self.headroom_percent();
        if h > 0 {
            let v = self.apply_headroom(h, false).await;
            eprintln!(
                "slot: headroom {h}% of the {}: {} of {} tokens, cpu quota {}",
                v.headroom_base.as_str(),
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
                if !g.active.values().any(|l| l.started() && l.pid.is_none()) {
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
            done_output,
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
        for ((r, _, ring), ends) in running.iter_mut().zip(ends) {
            r.output = ring.snapshot();
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
            done_output: done_output
                .into_iter()
                .map(|(id, ring)| (id, ring.snapshot()))
                .collect(),
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
        for (r, conn, _) in &mut running {
            r.conn = conn.and_then(|c| index.get(&c).copied());
        }
        state.running = running.into_iter().map(|(r, _, _)| r).collect();
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
        let mut history = self.history.lock().expect("history lock");
        state.history = std::mem::take(&mut *history);
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
        drop(history);
        Err(Abort::Fatal(anyhow!("re-exec {}: {err}", exe.display())))
    }

    /// Takes the job table for a handover, under the server lock: running
    /// jobs with their pumps, queued jobs with their requests. Tokens that
    /// queued requests gathered go back to the FIFO.
    fn take_table(&self, g: &mut Inner) -> Table {
        let mut t = Table {
            next_id: g.next_id,
            done_output: g
                .done
                .iter()
                .rev()
                .take(DONE_OUTPUT_KEEP)
                .filter_map(|d| Some((d.job.id, Arc::clone(d.out.as_ref()?))))
                .collect(),
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
                        output: Vec::new(),
                        exited: l.exited,
                        conn: None,
                    },
                    l.conn,
                    Arc::clone(&l.out),
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
        // Every running job goes into the table before any supervisor starts
        // and before the first client is accepted. A job that ended during
        // the handover is finished as soon as its supervisor runs; with a
        // job still missing from the table, that finish would find no job
        // holding slots and the idle reconcile would fill the FIFO beside
        // the slots the missing jobs hold.
        let mut adopted = Vec::with_capacity(n_running);
        for r in state.running {
            let conn = r.conn.and_then(|i| conns.get_mut(i).and_then(Option::take));
            let out = h.take(r.stdout);
            let err = h.take(r.stderr);
            let (id, pid) = (r.job.id, r.pid);
            let (tx, gone, keep) = self.adopt_running(r, out, err, conn.as_ref(), &snap);
            adopted.push((id, pid, tx, gone, keep, conn));
        }
        for (id, pid, tx, gone, keep, conn) in adopted {
            let s = Arc::clone(self);
            tokio::spawn(async move {
                let _keep = keep;
                let done = s.supervise(id, pid, &tx, gone, false).await;
                drop(tx);
                s.end_conn(conn, done.is_some()).await;
            });
        }
        // The running jobs are in the table: admission by share counts them
        // (by their whole share, no figure yet) before a queued job is let in.
        self.apply_saved_admission();
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
        let ring = Arc::new(Ring::from_tail(OUTPUT_KEEP, r.output));
        let pumps = self.start_pumps([out, err], &tx, &ring, &r.rewrite);
        {
            let mut g = self.inner.lock().expect("server lock");
            let expect_rss = g
                .memory
                .expected(&r.job.repo, &r.job.argv, &r.job.toolchain);
            let expect_cores = self.expected_cores(&g.memory, &r.job.repo, &r.job.argv);
            let mut tracker = Tracker::default();
            tracker.sample(snap, r.pid, &[], 0.0);
            g.active.insert(
                id,
                Live {
                    // The record carries the share it asked for by share;
                    // by slots it asked for its slots.
                    share: r.job.share.unwrap_or(f64::from(r.job.tokens.max(1))),
                    claimed: true,
                    spillable: false,
                    env_target: crate::evict::pid_env_target(r.pid, Path::new(&r.job.cwd)),
                    job: r.job,
                    pid: Some(r.pid),
                    held: r.held,
                    tracker,
                    out: ring,
                    cores: 0.0,
                    rss: 0,
                    procs: 0,
                    expect_rss,
                    expect_cores,
                    recent: cpu_limit::Recent::default(),
                    cpu_wait: false,
                    cpu_wait_since: None,
                    stop,
                    req: None,
                    waiter: None,
                    pumps,
                    rewrite: r.rewrite,
                    exited: r.exited,
                    conn: conn.map(|a| a.id),
                    target_wait: false,
                    // The job record carries its limit and its start: the
                    // watch counts on from there.
                    overdue: watch::Sender::new(false),
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

    /// Fetch once (`cargo_fetch.rs`) for a cargo job of workspace `id` that
    /// runs in `cwd`: whether the job runs offline, or why it goes back to
    /// the client.
    async fn fetch_once(
        &self,
        id: &str,
        root: &Path,
        cwd: &Path,
        argv: &[String],
        env: &[String],
        tx: &mpsc::UnboundedSender<Frame>,
    ) -> Result<bool, String> {
        use crate::cargo_fetch::{Check, Outcome, cargo_of, ensure, label, locked};
        let ws_root = self.cfg.ws_root.clone();
        let root = root.to_path_buf();
        let id = id.to_string();
        let cwd = cwd.to_path_buf();
        let cargo = cargo_of(argv);
        let locked = locked(argv);
        let what = label(locked);
        let env = env.to_vec();
        let out = tokio::task::spawn_blocking(move || {
            ensure(&Check {
                ws_root: &ws_root,
                root: &root,
                id: &id,
                cwd: &cwd,
                cargo,
                env: &env,
                locked,
                limit: crate::cargo_fetch::LIMIT,
            })
        })
        .await
        .map_err(|e| format!("{what} failed: {e}"))??;
        Ok(match out {
            Outcome::NoLock => false,
            Outcome::Current => true,
            Outcome::Fetched(t) => {
                let _ = tx.send(Frame::Info {
                    msg: format!(
                        "slot: new Cargo.lock on this mirror: {what} took {:.1} s; its cargo jobs run offline",
                        t.as_secs_f64()
                    ),
                });
                true
            }
        })
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

    /// Where workspace `ws` is mirrored: `<ws-root>/<id>`, or for a
    /// same-path mirror the client's own root, which must lie below a
    /// `--same-path-root`.
    fn tree(&self, ws: &crate::proto::Workspace) -> Result<PathBuf> {
        if !ws.same_path {
            return Ok(self.cfg.ws_root.join(&ws.id));
        }
        let root = Path::new(&ws.client_root);
        if self.below_same_path_root(root) {
            return Ok(root.to_path_buf());
        }
        bail!(
            "same-path mirror `{}` is not below a --same-path-root of this server",
            ws.client_root
        )
    }

    /// Whether `p` is absolute, without `.` or `..`, and below one of the
    /// `--same-path-root` directories.
    fn below_same_path_root(&self, p: &Path) -> bool {
        crate::same_path::plain(p)
            && self
                .cfg
                .same_path_roots
                .iter()
                .any(|r| p.starts_with(r) && p != r)
    }

    /// A same-path job's own target directory (`Workspace::target_dir`),
    /// which must lie below a `--same-path-root`; `None` when it names none.
    fn same_path_target(&self, ws: &crate::proto::Workspace) -> Result<Option<PathBuf>> {
        if !ws.same_path || ws.target_dir.is_empty() {
            return Ok(None);
        }
        let t = Path::new(&ws.target_dir);
        if self.below_same_path_root(t) {
            return Ok(Some(t.to_path_buf()));
        }
        bail!(
            "target directory `{}` is not below a --same-path-root of this server",
            ws.target_dir
        )
    }

    /// Where the job of workspace `ws` builds: `<cache>/target/<id>`, or a
    /// same-path job's own target directory (else `target` in its tree).
    fn job_target(&self, ws: &crate::proto::Workspace) -> PathBuf {
        if ws.same_path {
            if let Ok(Some(t)) = self.same_path_target(ws) {
                return t;
            }
            if let Ok(t) = self.tree(ws) {
                return t.join("target");
            }
        }
        self.cfg.cache_root.join("target").join(&ws.id)
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
        let root = self.tree(ws)?;
        self.same_path_target(ws)?;
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
        mut req: Request,
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
        let mut base = Self::base_env(&req);
        if !req.argv[0].contains('/')
            && let Err(e) = look_path(&req.argv[0], &base)
        {
            return self.reply(&tx, 127, Some(format!("slot: {e}; not queued")));
        }
        if req.fetch_once
            && let Some(ws) = req.workspace.as_ref()
            && crate::cargo_fetch::applies(&req.argv)
        {
            let root = PathBuf::from(&from);
            match self
                .fetch_once(&ws.id, &root, &cwd, &req.argv, &base, &tx)
                .await
            {
                Ok(offline) => {
                    if offline && !req.env.iter().any(|e| e == crate::cargo_fetch::OFFLINE) {
                        req.env.push(crate::cargo_fetch::OFFLINE.to_string());
                        base = Self::base_env(&req);
                    }
                }
                Err(msg) => {
                    let g = self.inner.lock().expect("server lock");
                    if g.handing_over {
                        return None;
                    }
                    let _ = tx.send(Frame::Fallback { msg });
                    return Some(());
                }
            }
        }
        // A gated job is shown, counted and sized by the command its client
        // runs in a sandbox; the daemon starts only `req.argv`, the hold.
        let shown = req
            .hold_for
            .clone()
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| req.argv.clone());
        let tc = toolchain::for_argv(&shown, self.cfg.go_tokens);
        let limit = self.pool.limit();
        let tokens = job_tokens(tc.tokens(), req.tokens, self.pool.size(), limit);
        // By share a job asks for at most the budget as it is now: a share
        // over it would never fit the free share.
        let share_cap = self.share_ask_cap();
        if self.shares_on() {
            if let Some(n) = req.tokens.filter(|&n| f64::from(n) > share_cap) {
                let _ = tx.send(Frame::Info {
                    msg: format!(
                        "slot: --tokens {n} capped at a share of {share_cap:.1} CPUs, the budget"
                    ),
                });
            }
        } else if let Some(n) = req.tokens.filter(|&n| n as usize > limit) {
            let _ = tx.send(Frame::Info {
                msg: format!("slot: --tokens {n} capped at {limit}, the slots in circulation"),
            });
        }
        // Said once: a job a handover queues again was told by the old image.
        if resume.is_none() {
            let limits = *self.run_limit.lock().expect("run limit lock");
            let max = limits.max_s();
            if let Some(asked) = req.timeout_s.filter(|&t| t > max) {
                let _ = tx.send(Frame::Info {
                    msg: run_limit::capped_message(asked, max),
                });
            }
            // The limit as of now; the job's own is fixed when it starts.
            let limit = run_limit::job_limit(req.timeout_s, limits.run_limit_s, max);
            if let Some(msg) = run_limit::wrapped_message(limit, &shown, max) {
                let _ = tx.send(Frame::Info { msg });
            }
        }
        let target_dir = (tc.name() == "rust" && req.workspace.is_none())
            .then(|| crate::evict::target_dir(&shown, &req.env, &cwd))
            .flatten();
        // What the job's environment says about the compile cache. The
        // toolchain adds none of these variables, so `base` has them as the
        // job gets them.
        let cache_env = (tc.name() == "rust").then(|| crate::sccache::cache_env(&base));
        let warnings = cache_env
            .as_ref()
            .map(crate::sccache::cache_warnings)
            .unwrap_or_default();
        let project = crate::project::cached(&cwd).unwrap_or_default();
        let out = Arc::new(Ring::new(OUTPUT_KEEP));
        let (id, share) = {
            let mut g = self.inner.lock().expect("server lock");
            let (id, enqueued_ms) = if let Some(r) = resume {
                r
            } else {
                g.next_id += 1;
                (g.next_id, now_ms())
            };
            let expect_rss = g.memory.expected(&req.repo, &shown, tc.name());
            let expect_cores = self.expected_cores(&g.memory, &req.repo, &shown);
            let (priority, promoted) = self.level_of(&req.repo, req.priority, enqueued_ms);
            let predicted_ms = g.memory.expected_runtime(&req.repo, &shown);
            // A client that names slots asks for that share: one of 0.21.0
            // and older knows no other way to ask.
            let share = crate::shares::share_for(
                req.tokens,
                g.memory.predicted_share(&req.repo, &shown, now_ms()),
                share_cap,
            );
            let job = Job {
                id,
                argv: shown.clone(),
                cwd: cwd.to_string_lossy().into_owned(),
                repo: req.repo.clone(),
                checkout: req.checkout.clone(),
                project: project.clone(),
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
                local_reason: spill::local_reason(&req, self.remotes.spills()),
                target_dir: target_dir
                    .as_ref()
                    .map(|t| t.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                priority,
                promoted,
                cache_env,
                warnings: warnings.clone(),
                limit_s: None,
                timed_out: false,
                predicted_ms,
                collected: 0,
                ahead: false,
                queue_class: None,
                share: self.shares_on().then_some(round2(share)),
                share_used: None,
                lent_s: 0.0,
                admission: crate::shares::Mode::Slots,
                node: LOCAL_NODE.to_string(),
                from_node: false,
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
                    expect_cores,
                    recent: cpu_limit::Recent::default(),
                    cpu_wait: false,
                    cpu_wait_since: None,
                    stop,
                    req: Some(req.clone()),
                    waiter: None,
                    pumps: Vec::new(),
                    rewrite: (from.clone(), to.clone()),
                    exited: None,
                    conn,
                    target_wait: false,
                    overdue: watch::Sender::new(false),
                    share,
                    claimed: false,
                    spillable: false,
                    env_target: crate::evict::env_target(&req.env, &cwd),
                },
            );
            (id, share)
        };
        if crate::project::cached(&cwd).is_none() {
            // git runs off the request path: a job is queued without waiting for it.
            let s = Arc::clone(self);
            let dir = cwd.clone();
            std::thread::spawn(move || {
                let p = crate::project::resolve(&dir);
                if let Some(l) = s.inner.lock().expect("server lock").active.get_mut(&id) {
                    l.job.project = p;
                }
            });
        }
        self.publish(id);
        if let Some(ws) = &req.workspace {
            self.prepare_cache(&ws.id, &req.repo, &tx).await;
        }

        let ahead = self.pool.waiting_before(self.queued_level(id));
        if let Some(room) = self.pool.shares() {
            if ahead > 0 || room.free < share || room.running >= room.max {
                let _ = tx.send(Frame::Info {
                    msg: format!(
                        "slot: job {id} queued for a share of {share:.1} CPUs: {:.1} free, \
                         {ahead} request(s) ahead",
                        room.free.max(0.0)
                    ),
                });
            }
        } else {
            let free = self.pool.free();
            if ahead > 0 || free < tokens {
                let _ = tx.send(Frame::Info {
                    msg: format!(
                        "slot: job {id} queued for {tokens} slot(s): {free} free, {ahead} request(s) ahead"
                    ),
                });
            }
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
        // Whether the job compiles into a cargo target directory, which
        // another job may be compiling into.
        let shares_target = target_dir.is_some() && crate::cargo_lock::compiles(&shown);
        let mut told_target = false;
        // Whether the job may go to a remote node of the cluster file.
        let mut may_spill = self.spill_eligible(id, &req);
        let held = loop {
            if may_spill && let Some(t) = self.spill_pick(id) {
                match self
                    .run_spilled(id, &req, &cwd, t, &tx, gone.clone(), &out)
                    .await?
                {
                    Ran::Requeue => {
                        may_spill = false;
                        continue;
                    }
                    Ran::Finished => return Some(()),
                }
            }
            if !self.admission_open(gone.clone()).await {
                return self.finish(id, &tx, cancelled).ok();
            }
            if !self.resources_ready(id, &disks, &tx, gone.clone()).await {
                return self.finish(id, &tx, cancelled).ok();
            }
            if shares_target
                && !self
                    .target_ready(id, &tx, gone.clone(), &mut told_target)
                    .await
            {
                return self.finish(id, &tx, cancelled).ok();
            }
            // Queued under the server lock, so a re-rank sees the waiter.
            // The job's id is its arrival: after a wait outside the queue
            // it stands where its age puts it. Its place inside its level
            // comes from its predicted run time and its wait.
            let mut waiter = {
                let mut g = self.inner.lock().expect("server lock");
                let (level, place) = g
                    .active
                    .get(&id)
                    .map_or((Priority::Normal, jobserver::Place::default()), |l| {
                        (l.job.priority, self.place_of(&l.job))
                    });
                // What it asks for by share; by slots the pool reads only
                // `tokens`.
                let ask = jobserver::Ask {
                    share,
                    brief: predicted_short(g.active.get(&id).and_then(|l| l.job.predicted_ms)),
                };
                let w = self.pool.acquire_share(
                    tokens,
                    level,
                    u64::try_from(id).unwrap_or(0),
                    place,
                    ask,
                );
                if let Some(l) = g.active.get_mut(&id) {
                    l.waiter = Some(w.handle());
                    l.target_wait = false;
                }
                w
            };
            // While it waits for slots it keeps looking at its target
            // directory: once another job compiles there, or one that is
            // served before it queues for the directory, it leaves the
            // queue, so it gathers no slot it could not use.
            let in_use = loop {
                tokio::select! {
                    () = waiter.ready() => break false,
                    () = client_gone(gone.clone()) => {
                        if self.pool.cancel(&waiter) {
                            self.pool.release(waiter.take_held());
                        }
                        return self.finish(id, &tx, cancelled).ok();
                    }
                    () = tokio::time::sleep(self.cfg.target_poll), if shares_target => {
                        if self.target_wait(id, false).is_some() {
                            break true;
                        }
                    }
                    // A node has room for it: it leaves the queue and goes.
                    () = tokio::time::sleep(spill::TICK), if may_spill => {
                        if self.spill_due(id) && self.remotes.has_room(now_ms()) {
                            break true;
                        }
                    }
                }
            };
            if in_use {
                if self.pool.cancel(&waiter) {
                    self.pool.release(waiter.take_held());
                }
                self.left_queue(id);
                continue;
            }
            // Memory may have gone while the job waited for slots: give
            // them back and wait for memory first. The same for its target
            // directory.
            if self.short_of(id, &disks).is_some() || self.target_wait(id, false).is_some() {
                self.pool.release(waiter.take_held());
                self.left_queue(id);
                continue;
            }
            // Claim the slots under the server lock: an upgrade requested
            // meanwhile (hold or handover) sends them back and the job waits
            // with the others; a handover sees the claim and waits for the
            // process. Under the same lock a job sees the claim of another
            // job of its target directory, and one served before it that
            // has its slots and no claim yet: of two that got slots at the
            // same moment the first in the queue starts and the other waits.
            // The CPU limit is weighed under that lock too: of the jobs
            // held for the host's CPUs only as many claim as the figure
            // admits, each counting the ones that claimed before it.
            let cpu_limits = self.cpu_limits();
            let claimed = {
                let mut g = self.inner.lock().expect("server lock");
                let settle = self.cfg.target_settle.as_millis() as i64;
                let taken = target_use(&g.active, id, now_ms(), settle)
                    .is_some_and(|u| u.starting.is_some() || u.ahead.is_some());
                let cpu = self.cpu_verdict(&g, cpu_limits, id, now_ms());
                let cpu_held = matches!(cpu, cpu_limit::Verdict::Held(_));
                if *self.upgrading.borrow() || g.handing_over || taken || cpu_held {
                    if let Some(l) = g.active.get_mut(&id) {
                        l.waiter = None;
                    }
                    None
                } else {
                    let n = waiter.take_held();
                    let ahead = waiter.ahead();
                    if cpu == cpu_limit::Verdict::Overdue {
                        g.cpu_bound_ms = now_ms();
                    }
                    g.active.get_mut(&id).map(|l| {
                        l.held = n;
                        l.claimed = true;
                        // Only admission by share grants a request no
                        // token; by slots every request takes one or more.
                        if n == 0 {
                            l.job.admission = crate::shares::Mode::Shares;
                        }
                        // By share the job waited in the queue with a
                        // reason; by slots it has none here.
                        l.job.wait_reason.clear();
                        if l.spillable && l.job.local_reason.is_empty() {
                            l.job.local_reason = spill::HERE_FIRST.into();
                        }
                        l.waiter = None;
                        l.cpu_wait = false;
                        l.job.collected = 0;
                        l.job.ahead |= ahead;
                        l.job.queue_class = None;
                        (n, cpu)
                    })
                }
            };
            match claimed {
                Some((n, cpu)) => {
                    match cpu {
                        cpu_limit::Verdict::Overdue => self.cpu_hold_ran_out(id, &tx),
                        cpu_limit::Verdict::Short(ms) => self.cpu_hold_passed(id, ms, &tx),
                        cpu_limit::Verdict::Free | cpu_limit::Verdict::Held(_) => {}
                    }
                    break n;
                }
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
        // A same-path job builds where the client would, so the paths cargo
        // prints hold on both machines.
        if let Some(ws) = req.workspace.as_ref().filter(|w| w.same_path) {
            env = match self.same_path_target(ws) {
                Ok(Some(t)) => toolchain::set(env, "CARGO_TARGET_DIR", &t.to_string_lossy()),
                _ => toolchain::unset(env, &["CARGO_TARGET_DIR"]),
            };
        }
        // The bound in force at the start is the job's, like its run-time
        // limit: a later `slot job-parallel` changes the jobs that start
        // after it. With a bound the job gets no pool jobserver.
        let own =
            job_parallel::for_job(*self.job_parallel.lock().expect("job parallel lock"), held);
        if let Some(p) = own {
            env = toolchain::own_jobserver(env, p);
        }
        env.push(format!("{JOB_ENV}={id}"));

        let mut argv = req.argv.clone();
        // Inside the memory scope: `systemd-run` needs the user bus, which
        // the sandbox hides.
        if let Some(sandbox) = &self.cfg.sandbox {
            env = crate::sandbox::Sandbox::env(env);
            let workspace = req.workspace.as_ref().and_then(|w| self.tree(w).ok());
            let target_dir = match &req.workspace {
                Some(w) => Some(self.job_target(w)),
                None => crate::evict::target_dir(&shown, &env, &cwd),
            };
            let job = crate::sandbox::Job {
                cwd: &cwd,
                workspace: workspace.as_deref(),
                target_dir,
                env: &env,
                ws_root: &self.cfg.ws_root,
                cache_root: &self.cfg.cache_root,
                socket: &self.cfg.socket,
                fifo: self.pool.path(),
            };
            sandbox.prepare(&job);
            argv = sandbox.wrap(&job, argv);
        }
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
        // A job whose open-file limit is not the daemon's starts as `slot
        // __exec`, which sets the limit in the job's own process and then
        // executes the program in its place: the job has the limit from its
        // first instruction, and nothing runs between fork and exec here.
        let nofile = job_file_limit(&req, &tx);
        let mut cmd = if let (Some(limit), Some(shim)) = (nofile, &self.cfg.exec_shim) {
            let mut c = Command::new(shim);
            c.arg0("slot")
                .arg("__exec")
                .arg("--nofile")
                .arg(limit.arg())
                .arg("--arg0")
                .arg(&argv[0])
                .arg("--")
                .arg(&program)
                .args(&argv[1..]);
            c
        } else {
            let mut c = Command::new(&program);
            c.arg0(&argv[0]).args(&argv[1..]);
            c
        };
        cmd.current_dir(&cwd)
            .env_clear()
            .envs(env.iter().filter_map(|e| e.split_once('=')))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        // Sent as the job starts, ahead of anything the command prints. An
        // `info` frame, so a client of any version prints it.
        for w in &warnings {
            let _ = tx.send(Frame::Info {
                msg: format!("slot: warning: {w}"),
            });
        }
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
            self.job_target(w).to_string_lossy().into_owned()
        });
        // With fixed paths a synced workspace's job names its tree by the
        // fixed path; its output is rewritten from there.
        let from = match &self.cfg.sandbox {
            Some(s) if s.fixed() && req.workspace.is_some() => {
                crate::sandbox::FIXED_SRC.to_string()
            }
            _ => from,
        };
        let pumps = self.start_pumps(pipes, &tx, &out, &(from, to));
        // The limit in force at the start is the job's: a later `slot
        // run-limit` changes the jobs that start after it.
        let limit_s = {
            let limits = self.run_limit.lock().expect("run limit lock");
            run_limit::job_limit(req.timeout_s, limits.run_limit_s, limits.max_s())
        };
        {
            let mut g = self.inner.lock().expect("server lock");
            if let Some(l) = g.active.get_mut(&id) {
                l.pid = Some(pid);
                l.job.started_ms = now_ms();
                l.job.limit_s = limit_s;
                l.job.state = JobState::Running;
                l.tracker.sample(&Snapshot::scan(), pid, &[], 0.0);
                l.req = None;
                l.pumps = pumps;
            }
            let _ = tx.send(Frame::Start { id, target_dir });
        }
        self.publish(id);
        let done = self.supervise(id, pid, &tx, gone, false).await.map(drop);
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
        requeue: bool,
    ) -> Option<Ran> {
        let s = Arc::clone(self);
        let wait = tokio::task::spawn_blocking(move || s.reap(id, pid));
        tokio::pin!(wait);
        let overdue = {
            let g = self.inner.lock().expect("server lock");
            g.active.get(&id).map(|l| l.overdue.subscribe())
        };
        let mut cancelled = false;
        // A job past its run-time limit ends the way a cancelled one does.
        let stopped = tokio::select! {
            r = &mut wait => Some(r),
            () = client_gone(gone.clone()) => {
                cancelled = true;
                None
            }
            () = past_limit(overdue) => None,
        };
        let result = if let Some(r) = stopped {
            r
        } else {
            let _ = killpg(Pid::from_raw(pid), Signal::SIGTERM);
            if let Ok(r) = tokio::time::timeout(Duration::from_secs(5), &mut wait).await {
                r
            } else {
                let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
                wait.await
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
        // Read from the job record, not from the branch taken above: an
        // image that took the job over reaps what the old one signalled.
        let timed_out = {
            let g = self.inner.lock().expect("server lock");
            g.active
                .get(&id)
                .filter(|l| l.job.timed_out)
                .map(|l| (now_ms() - l.job.started_ms, l.job.limit_s.unwrap_or(0)))
        };
        let end = if let Some((ran_ms, limit)) = timed_out {
            let max = self.run_limit.lock().expect("run limit lock").max_s();
            End {
                state: JobState::Failed,
                code: run_limit::EXIT,
                cpu: exited.cpu,
                client_code: run_limit::EXIT,
                msg: Some(run_limit::ended_message(id, ran_ms, limit, max)),
                note_wait: false,
            }
        } else if cancelled {
            End {
                state: JobState::Cancelled,
                code: -1,
                cpu: exited.cpu,
                client_code: 130,
                msg: None,
                note_wait: false,
            }
        } else if requeue && exited.code == spill::INFRA_EXIT {
            return Some(Ran::Requeue);
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
        self.finish(id, tx, end).ok().map(|()| Ran::Finished)
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
                    .or_else(|| {
                        if self.cfg.same_path_roots.is_empty() {
                            return None;
                        }
                        crate::mirror::linked_id(&self.cfg.ws_root, Path::new(&l.job.cwd))
                    })
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
        // A same-path job builds at its tree's own path; another tree's target
        // carries freshness records and build-script output made for that
        // tree, which cargo may take as fresh here. Such a job is not seeded.
        let same_path = !self.cfg.same_path_roots.is_empty();
        let Ok((gone, seeded)) = tokio::task::spawn_blocking(move || {
            cache.touch(&ws, &repo, now_ms());
            let gone = cache.evict(budget, floor, &busy);
            let seeded = if same_path {
                Err("same-path workspace".to_string())
            } else {
                cache.seed(&ws, &repo, &busy)
            };
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
    /// of space, memory is short and another job runs, or the host is
    /// busier than the mark of `slot cpu-limit`. `(kind, text)`.
    fn short_of(&self, id: i64, disks: &[PathBuf]) -> Option<(&'static str, String)> {
        let limits = self.cfg.admission;
        let disk_free = self.cfg.disk_free.unwrap_or(admission::disk_free);
        let free: Vec<(String, u64)> = disks
            .iter()
            .filter_map(|p| disk_free(p).map(|f| (p.display().to_string(), f)))
            .collect();
        if let Some(r) = admission::disk_wait(&free, limits) {
            // Eviction starts where the disk hold starts: its mark is at
            // least `--min-free-disk`.
            self.evict_wake.notify_one();
            return Some(("disk", r));
        }
        if let Some(r) = self.short_of_memory(id) {
            return Some(("memory", r));
        }
        let cpu = self.cpu_limits();
        cpu.cpu_busy_max?;
        let g = self.inner.lock().expect("server lock");
        match self.cpu_verdict(&g, cpu, id, now_ms()) {
            cpu_limit::Verdict::Held(text) => Some(("cpu", text)),
            cpu_limit::Verdict::Free
            | cpu_limit::Verdict::Overdue
            | cpu_limit::Verdict::Short(_) => None,
        }
    }

    /// Why job `id` may not start now as far as memory goes: it is short
    /// and another job runs.
    fn short_of_memory(&self, id: i64) -> Option<String> {
        let limits = self.cfg.admission;
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
        admission::memory_wait(available, reserved, expected, limits, others)
    }

    /// Waits until job `id` may start as far as disk, memory and the host's
    /// CPUs go ([`Server::short_of`]), telling
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
            let cpu = short.as_ref().is_some_and(|(kind, _)| *kind == "cpu");
            let now = now_ms();
            let (changed, first_cpu) = {
                let mut g = self.inner.lock().expect("server lock");
                g.active.get_mut(&id).map_or((false, false), |l| {
                    let c = l.job.wait_reason != reason;
                    l.job.wait_reason.clone_from(&reason);
                    // Let through after a hold, it stays weighed until it
                    // claims its slots; held for something else, it is not.
                    l.cpu_wait = cpu || (l.cpu_wait && short.is_none());
                    // Held for disk or memory, the time does not count
                    // towards the CPU hold's bound: its clock starts again.
                    if short.is_some() && !cpu {
                        l.cpu_wait_since = None;
                    }
                    let first = cpu && l.cpu_wait_since.is_none();
                    if first {
                        l.cpu_wait_since = Some(now);
                    }
                    (c, first)
                })
            };
            if changed {
                self.publish(id);
            }
            if first_cpu {
                self.census_event(&serde_json::json!({
                    "t": now,
                    "event": "cpu_hold",
                    "job": id,
                    "reason": reason,
                }));
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
                () = tokio::time::sleep(self.cfg.resource_poll) => {}
                () = client_gone(gone.clone()) => return false,
            }
        }
    }

    /// Why job `id` may not start now as far as its cargo target directory
    /// goes: another job of the pool compiles into it, so cargo would only
    /// block on the build directory's lock and the job would hold its slots
    /// idle. That is a job about to compile ([`TargetUse::starting`]) or a
    /// running one while the lock is held ([`crate::cargo_lock::held`]):
    /// cargo gives the lock back before it runs tests, and from then on the
    /// directory is free. A queued job of the same directory that is served
    /// before it goes first too ([`TargetUse::ahead`]), so the builds of one
    /// directory start by level, then age, and only the first of them
    /// stands in the queue for slots. `at_gate` marks the job as at the
    /// gate. `None` for a job that is not a local cargo build, or with
    /// nothing in its way.
    fn target_wait(&self, id: i64, at_gate: bool) -> Option<String> {
        let settle = self.cfg.target_settle.as_millis() as i64;
        let u = {
            let mut g = self.inner.lock().expect("server lock");
            if at_gate && let Some(l) = g.active.get_mut(&id) {
                l.target_wait = true;
            }
            target_use(&g.active, id, now_ms(), settle)?
        };
        // The lock is read outside the server lock: it is file I/O.
        let user = u
            .starting
            .or_else(|| u.running.filter(|_| crate::cargo_lock::held(&u.dir)));
        if let Some(job) = user {
            return Some(format!("target directory in use by job {job}"));
        }
        u.ahead
            .map(|job| format!("target directory goes to job {job} first"))
    }

    /// Job `id` left the queue for slots without starting.
    fn left_queue(&self, id: i64) {
        let mut g = self.inner.lock().expect("server lock");
        if let Some(l) = g.active.get_mut(&id) {
            l.waiter = None;
        }
    }

    /// Waits until no other job compiles into the cargo target directory of
    /// job `id` ([`Server::target_wait`]), outside the queue for slots, so
    /// the jobs behind it start meanwhile. Tells the client once why it
    /// waits (`told`); false when the client went away first.
    async fn target_ready(
        &self,
        id: i64,
        tx: &mpsc::UnboundedSender<Frame>,
        gone: watch::Receiver<bool>,
        told: &mut bool,
    ) -> bool {
        loop {
            let wait = self.target_wait(id, true);
            let reason = wait.clone().unwrap_or_default();
            let changed = {
                let mut g = self.inner.lock().expect("server lock");
                g.active.get_mut(&id).is_some_and(|l| {
                    let c = l.job.wait_reason != reason;
                    l.job.wait_reason.clone_from(&reason);
                    // It waits outside the queue: the jobs held for the
                    // host's CPUs behind it do not wait for it.
                    l.cpu_wait &= wait.is_none();
                    c
                })
            };
            if changed {
                self.publish(id);
            }
            let Some(text) = wait else {
                return true;
            };
            if !*told {
                let _ = tx.send(Frame::Info {
                    msg: format!("slot: job {id} waiting: {text}"),
                });
                eprintln!("slot: job {id} waiting: {text}");
                *told = true;
            }
            tokio::select! {
                () = tokio::time::sleep(self.cfg.target_poll) => {}
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
            let ran_ms = live.job.ended_ms - live.job.started_ms;
            if live.job.share.is_some() && live.job.started_ms > 0 && ran_ms > 0 {
                live.job.share_used = Some(round2(live.job.cpu_s * 1000.0 / ran_ms as f64));
            }
            // The budget of admission by share moves on how much slower
            // than their best the jobs that ended ran.
            if self.shares_on()
                && live.job.started_ms > 0
                && matches!(end.state, JobState::Ok | JobState::Failed)
                && let Some(s) = crate::shares::slowdown(
                    ran_ms,
                    g.memory.best_runtime(&live.job.repo, &live.job.argv),
                )
            {
                g.budget.finished(s);
            }
            let job = live.job.clone();
            g.metrics.record(&job);
            // A job that ran on a node says nothing of this machine.
            if job.node == LOCAL_NODE {
                g.memory.record(&job);
            }
            record_target(&mut g.targets, &job);
            record_ran(&mut g.ran_in, &job);
            g.tally.record(&job, job.ended_ms);
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

    /// Reads `/proc` once a second: the census, the jobs' CPU and the machine quota.
    async fn sampler(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut scans = ScanInterval(Instant::now());
        // The census counts no CPU time in its first sample: it has no
        // earlier reading to compare with.
        let mut primed = false;
        loop {
            tick.tick().await;
            let (base, percent) = {
                let h = self.headroom.lock().expect("headroom lock");
                (h.base, h.percent)
            };
            // The cgroup tree is read only while the machine base is in force.
            let own = (base == HeadroomBase::Machine && percent > 0)
                .then(|| self.own_cgroup.clone())
                .flatten();
            let uid = self.uid;
            let Ok((at, snap, sys, cgroups)) = tokio::task::spawn_blocking(move || {
                (
                    Instant::now(),
                    Snapshot::scan(),
                    procstat::system_cpu(),
                    own.map(|o| machine::read(Path::new(CGROUP_ROOT), uid, &o)),
                )
            })
            .await
            else {
                continue;
            };
            let dt = scans.next(at);
            let mut new_ids = Vec::new();
            let mut machine_quota = None;
            let share_changed: Vec<i64>;
            let event = {
                let mut g = self.inner.lock().expect("server lock");
                let (pb, pt) = g.sys_prev;
                if pt > 0 && sys.1 > pt {
                    g.sys_cpu = (sys.0 - pb) as f64 / (sys.1 - pt) as f64;
                }
                g.sys_prev = sys;
                if let Some(r) = cgroups {
                    let busy = g.sys_cpu * f64::from(self.cores);
                    if let Some(now) = g.machine.sample(&r, busy, dt) {
                        g.machine_shown = machine::shown(now, self.cores, percent);
                        machine_quota = Some(g.machine_shown.quota);
                    }
                } else {
                    g.machine = machine::Monitor::default();
                    g.machine_shown = machine::Shown::default();
                }
                let extra = compile_server_work(&mut g, &snap);
                let mut jobs = Vec::new();
                let mut total = 0.0;
                for (id, l) in &mut g.active {
                    let Some(pid) = l.pid else { continue };
                    let s =
                        l.tracker
                            .sample(&snap, pid, extra.get(id).map_or(&[], Vec::as_slice), dt);
                    l.cores = s.cores;
                    l.recent.push(s.cores, now_ms());
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
                // The compile cache servers outside every job; their
                // compilers that carry a job's tag are in `managed` and
                // count for the pool, not twice.
                let cache = census::compile_cache_pids(&snap, COMPILE_SERVERS, &managed);
                let sample = g.census.sample(&snap, &managed, &cache, dt);
                if primed {
                    g.host_busy.push(sample.parts(), now_ms());
                }
                primed = true;
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
                g.psi_cpu = sample.pressure.cpu;
                // Admission by share looks once a tick, with this sample.
                share_changed = self.share_step(&mut g, now_ms());
                let shares = self.shares_on().then_some(g.share_view);
                let m = g.machine_shown.clone();
                // Read before the literal: its guard would live to the end of
                // it, and `headroom_percent` and `call_apps` lock again.
                let quota = self.headroom.lock().expect("headroom lock").quota;
                Event::Tick {
                    shares,
                    cpu_quota_percent: quota,
                    headroom_base: base,
                    busy_cores: m.busy,
                    target_cores: m.target,
                    outside_cores: m.outside,
                    outside: m.owners,
                    quota_floor: m.floor,
                    over_target: m.over,
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
            for id in share_changed {
                self.publish(id);
            }
            if let Some(q) = machine_quota {
                self.plan_machine_quota(q);
            }
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
            unaccounted: self.unaccounted.load(Ordering::Relaxed),
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
            headroom_base: h.base,
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

    /// Applies a `PUT /api/limits` body: the base first, then the call
    /// headroom, then the headroom.
    pub async fn update_limits(self: Arc<Self>, req: limits::Request) -> limits::View {
        if let Some(b) = req.headroom_base {
            let changed = {
                let mut h = self.headroom.lock().expect("headroom lock");
                std::mem::replace(&mut h.base, b) != b
            };
            self.save_limits();
            if changed && req.headroom_percent.is_none() {
                let p = self.headroom_percent();
                self.apply_headroom(p, false).await;
            }
        }
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
        let run_limit = *self.run_limit.lock().expect("run limit lock");
        let job_parallel = *self.job_parallel.lock().expect("job parallel lock");
        let cpu = self.cpu_limits();
        let shares = *self.shares.lock().expect("shares lock");
        let saved = {
            let h = self.headroom.lock().expect("headroom lock");
            limits::Saved {
                headroom_percent: h.saved_percent,
                call_headroom_percent: h.call_saved,
                base: h.base,
                run_limit_s: run_limit.run_limit_s,
                run_limit_max_s: run_limit.run_limit_max_s,
                job_parallel,
                cpu_busy_max: cpu.cpu_busy_max,
                cpu_busy_floor: cpu.cpu_busy_floor,
                cpu_pressure_max: cpu.cpu_pressure_max,
                admission: shares.mode,
                share_floor: shares.floor,
                share_max: shares.max,
                share_pressure_max: shares.pressure_max,
                share_min_budget: shares.min_budget,
                evict_below_gb: *self.evict_below.lock().expect("evict below lock"),
                evict_idle_s: *self.evict_idle.lock().expect("evict idle lock"),
            }
        };
        if let Err(e) = limits::save(&self.cfg.state_dir, saved) {
            eprintln!("slot: save {}: {e}", limits::FILE);
        }
    }

    /// The level a queued job of `repo` that asked for `own` and queued at
    /// `enqueued_ms` is ordered by now, and whether it was promoted.
    fn level_of(&self, repo: &str, own: Option<Priority>, enqueued_ms: i64) -> (Priority, bool) {
        let now = (self.cfg.clock)();
        let waited = Duration::from_millis(u64::try_from(now - enqueued_ms).unwrap_or(0));
        let table = self.priority.lock().expect("priority lock");
        let promote_after = table.promote_after(self.cfg.promote_after);
        priority::level(&table.rules, repo, own, waited, promote_after)
    }

    /// The place inside its level of queued job `job`: whether later jobs
    /// of its level may still pass it (it has waited less than
    /// `--pass-for` since `enqueued_ms`), and whether it is short
    /// (`predicted_ms`). A job the pool already serves as one that may no
    /// longer be passed (`queue_class` 0) stays so: the wait is read on a
    /// wall clock, which can step back.
    fn place_of(&self, job: &Job) -> jobserver::Place {
        if job.queue_class == Some(0) {
            return jobserver::Place::default();
        }
        let now = (self.cfg.clock)();
        let waited = Duration::from_millis(u64::try_from(now - job.enqueued_ms).unwrap_or(0));
        priority::place(job.predicted_ms, waited, self.cfg.pass_for)
    }

    /// The level job `id` is queued at.
    fn queued_level(&self, id: i64) -> Priority {
        let g = self.inner.lock().expect("server lock");
        g.active
            .get(&id)
            .map_or(Priority::Normal, |l| l.job.priority)
    }

    /// Recomputes the level and the place of every queued job from the
    /// rules, its prediction and its wait, and reorders the pool's queue to
    /// match: the place changes without any event once a job has waited
    /// `--pass-for`. Then takes the slots each queued job has collected and
    /// whether it was moved ahead into its record.
    fn rerank(&self) {
        let changed: Vec<i64> = {
            let mut g = self.inner.lock().expect("server lock");
            let queued: Vec<(i64, String, Option<Priority>, i64)> = g
                .active
                .values()
                .filter(|l| l.job.state == JobState::Queued)
                .map(|l| {
                    (
                        l.job.id,
                        l.job.repo.clone(),
                        l.req.as_ref().and_then(|r| r.priority),
                        l.job.enqueued_ms,
                    )
                })
                .collect();
            let mut changed = Vec::new();
            for (id, repo, own, enqueued_ms) in queued {
                let (level, promoted) = self.level_of(&repo, own, enqueued_ms);
                let Some(l) = g.active.get_mut(&id) else {
                    continue;
                };
                if let Some(w) = &l.waiter {
                    let place = self.place_of(&l.job);
                    self.pool.set_place(w, level, place);
                }
                let (collected, ahead, class) = Self::queue_marks(l);
                if (
                    l.job.priority,
                    l.job.promoted,
                    l.job.collected,
                    l.job.ahead,
                    l.job.queue_class,
                ) != (level, promoted, collected, ahead, class)
                {
                    l.job.priority = level;
                    l.job.promoted = promoted;
                    l.job.collected = collected;
                    l.job.ahead = ahead;
                    l.job.queue_class = class;
                    changed.push(id);
                }
            }
            changed
        };
        for id in changed {
            self.publish(id);
        }
    }

    /// The slots a queued job has collected in the pool's queue, whether it
    /// was moved ahead there, and its class there. A job that waits outside
    /// the queue (for memory, disk or its target directory) collects none
    /// and keeps the class it had.
    fn queue_marks(l: &Live) -> (u32, bool, Option<u8>) {
        match &l.waiter {
            Some(w) => (
                u32::try_from(w.held()).unwrap_or(u32::MAX),
                l.job.ahead || w.ahead(),
                Some(w.class()),
            ),
            None => (0, l.job.ahead, l.job.queue_class),
        }
    }

    /// Every second: promotes queued jobs that waited past the promotion
    /// delay, makes those that waited `--pass-for` unpassable, and shows
    /// the slots queued jobs collected.
    async fn priority_watch(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            self.rerank();
        }
    }

    /// The priority rules and the promotion delay in force, as
    /// `GET /api/priority` answers.
    pub fn priority_view(&self) -> priority::View {
        let table = self.priority.lock().expect("priority lock");
        priority::View {
            rules: table.rules.clone(),
            promote_after_s: table.promote_after(self.cfg.promote_after).as_secs(),
        }
    }

    /// Sets or removes a priority rule (`slot priority set|unset`), sets the
    /// promotion delay (`slot priority promote-after`), saves both and
    /// re-ranks the queued jobs.
    pub fn set_priority(&self, req: &priority::Request) -> Result<priority::View, String> {
        if let Some(why) = req.refused() {
            return Err(why.into());
        }
        let saved = {
            let mut table = self.priority.lock().expect("priority lock");
            let mut changed = false;
            if let Some(pattern) = &req.pattern {
                changed |= priority::set(&mut table.rules, pattern.trim(), req.level);
            }
            if let Some(to) = req.promote_after_s {
                changed |= table.set_promote_after(to);
            }
            changed.then(|| table.clone())
        };
        if let Some(table) = saved {
            if let Err(e) = priority::save(&self.cfg.state_dir, &table) {
                eprintln!("slot: save {}: {e}", priority::FILE);
            }
            self.rerank();
        }
        Ok(self.priority_view())
    }

    /// The run-time limit and the most a job may name, as
    /// `GET /api/run-limit` answers.
    pub fn run_limit_view(&self) -> run_limit::View {
        self.run_limit.lock().expect("run limit lock").view()
    }

    /// Sets the run-time limit, the most a job may name, or both (`slot
    /// run-limit`) and saves them. Jobs that start afterwards get the new
    /// limit; a running job keeps the one it started with.
    pub fn set_run_limit(&self, req: &run_limit::Request) -> Result<run_limit::View, String> {
        let view = {
            let mut limits = self.run_limit.lock().expect("run limit lock");
            *limits = limits.changed(req)?;
            limits.view()
        };
        self.save_limits();
        Ok(view)
    }

    /// The limit on build processes per job, as `GET /api/job-parallel`
    /// answers.
    pub fn job_parallel_view(&self) -> job_parallel::View {
        job_parallel::View {
            job_parallel: *self.job_parallel.lock().expect("job parallel lock"),
        }
    }

    /// Sets or clears the limit on build processes per job (`slot
    /// job-parallel`) and saves it. Jobs that start afterwards get the new
    /// environment; a running job keeps the one it started with.
    pub fn set_job_parallel(
        &self,
        req: &job_parallel::Request,
    ) -> Result<job_parallel::View, String> {
        let to = req.setting()?;
        *self.job_parallel.lock().expect("job parallel lock") = to;
        self.save_limits();
        Ok(self.job_parallel_view())
    }

    /// The eviction mark and the idle keep in force and where they come
    /// from, as `GET /api/evict-below` answers.
    pub fn evict_below_view(&self) -> crate::evict::BelowView {
        crate::evict::BelowView::of(
            *self.evict_below.lock().expect("evict below lock"),
            self.cfg.evict.floor,
            self.cfg.admission.min_free_disk,
        )
        .with_idle(
            *self.evict_idle.lock().expect("evict idle lock"),
            self.cfg.evict.min_idle,
        )
    }

    /// Sets or forgets the eviction mark and the idle keep (`slot
    /// evict-below`), the ones the request names, saves them and makes
    /// eviction look at once.
    pub fn set_evict_below(
        &self,
        req: &crate::evict::BelowRequest,
    ) -> Result<crate::evict::BelowView, String> {
        let req = req.checked()?;
        if let Some(to) = req.evict_below_gb {
            *self.evict_below.lock().expect("evict below lock") = to;
        }
        if let Some(to) = req.evict_idle_s {
            *self.evict_idle.lock().expect("evict idle lock") = to;
        }
        self.save_limits();
        self.evict_set.notify_one();
        Ok(self.evict_below_view())
    }

    /// The mark and the floor of the CPU limit in force.
    fn cpu_limits(&self) -> cpu_limit::Limits {
        *self.cpu_limit.lock().expect("cpu limit lock")
    }

    /// The CPU limit, the host's busy CPUs and the jobs held for them, as
    /// `GET /api/cpu-limit` answers.
    pub fn cpu_limit_view(&self) -> cpu_limit::View {
        let limits = self.cpu_limits();
        let g = self.inner.lock().expect("server lock");
        let parts = match self.cpu_figure(&g, now_ms()) {
            cpu_limit::Figure::Busy(p) => Some(p),
            cpu_limit::Figure::Pending | cpu_limit::Figure::Unknown => None,
        };
        let held = g
            .active
            .values()
            .filter(|l| l.cpu_wait && !l.job.wait_reason.is_empty())
            .count();
        limits.view(self.cores, parts, held, Some(self.cpu_pressure(&g)))
    }

    /// CPU pressure of the host (`some avg10`, percent) as the census
    /// sampler last read it, or [`Config::cpu_pressure`] in tests.
    fn cpu_pressure(&self, g: &Inner) -> f64 {
        self.cfg.cpu_pressure.map_or(g.psi_cpu, |read| read())
    }

    /// Sets the CPU mark, the floor or the pressure mark, or switches the
    /// CPU limit off (`slot cpu-limit`), and saves them. A queued job is
    /// weighed against the new values the next time it looks
    /// ([`Config::resource_poll`]).
    pub fn set_cpu_limit(&self, req: &cpu_limit::Request) -> Result<cpu_limit::View, String> {
        {
            let mut limits = self.cpu_limit.lock().expect("cpu limit lock");
            *limits = limits.changed(req)?;
        }
        self.save_limits();
        Ok(self.cpu_limit_view())
    }

    /// Whether admission is by share.
    fn shares_on(&self) -> bool {
        self.shares.lock().expect("shares lock").mode == crate::shares::Mode::Shares
    }

    /// The most the budget of admission by share may reach: the pool's CPU
    /// quota where one is set, else the host's CPUs less the headroom.
    fn share_cap(&self) -> f64 {
        let (percent, quota) = {
            let h = self.headroom.lock().expect("headroom lock");
            (h.percent.min(limits::MAX_HEADROOM), h.quota)
        };
        let cores = f64::from(self.cores);
        let by_headroom = cores * f64::from(100 - percent) / 100.0;
        quota
            .map_or(by_headroom, |q| (f64::from(q) / 100.0).min(cores))
            .max(crate::shares::MIN_BUDGET)
    }

    /// Tokens that circulate with `percent` headroom: the pool less the
    /// headroom, and by share no more than the budget's CPUs. By share the
    /// build processes inside jobs are then bound to one per running job
    /// (each job's implicit token, which is in no FIFO) plus the budget's
    /// tokens.
    fn circulating(&self, percent: u8) -> usize {
        let by_headroom = limits::effective_tokens(self.cfg.tokens, percent);
        if !self.shares_on() {
            return by_headroom;
        }
        let budget = self.inner.lock().expect("server lock").budget.value();
        by_headroom.min(budget_tokens(budget))
    }

    /// The most share a job may ask for now: by share the budget as it is
    /// and the slots in circulation, never over [`Server::share_cap`].
    fn share_ask_cap(&self) -> f64 {
        let cap = self.share_cap();
        if !self.shares_on() {
            return cap;
        }
        let budget = self.inner.lock().expect("server lock").budget.value();
        cap.min(budget)
            .min(self.pool.limit() as f64)
            .max(crate::shares::MIN_SHARE)
    }

    /// The `GET /api/admission` answer.
    pub fn admission_view(&self) -> crate::shares::View {
        let settings = *self.shares.lock().expect("shares lock");
        let g = self.inner.lock().expect("server lock");
        if settings.mode == crate::shares::Mode::Shares
            && g.share_view.admission == crate::shares::Mode::Shares
        {
            crate::shares::View {
                floor: settings.floor(),
                max: settings.max(),
                pressure_max: settings.pressure_max(),
                ..g.share_view
            }
        } else {
            settings.view(self.cores)
        }
    }

    /// Puts the saved admission in force at once: the budget from the
    /// cap, and by share the pool's room and the FIFO's tokens, so no job
    /// is admitted by slots before the sampler's first look. Called when
    /// the daemon starts and once a handover's running jobs are in the
    /// table.
    fn apply_saved_admission(&self) {
        let changed = {
            let mut g = self.inner.lock().expect("server lock");
            if g.share_last_ms.is_none() {
                g.budget = crate::shares::Budget::new(self.cores, self.share_cap());
            }
            if !self.shares_on() {
                return;
            }
            self.share_step(&mut g, now_ms())
        };
        for id in changed {
            self.publish(id);
        }
    }

    /// Switches admission between slots and shares, or sets its bounds or
    /// its pressure mark (`slot admission`), saves them and looks at once.
    /// Switching to shares starts the budget anew.
    pub fn set_admission(
        &self,
        req: &crate::shares::Request,
    ) -> Result<crate::shares::View, String> {
        let switched_on = {
            let mut s = self.shares.lock().expect("shares lock");
            let next = s.changed(req)?;
            let on =
                s.mode == crate::shares::Mode::Slots && next.mode == crate::shares::Mode::Shares;
            *s = next;
            on
        };
        self.save_limits();
        let changed = {
            let mut g = self.inner.lock().expect("server lock");
            if switched_on {
                g.budget = crate::shares::Budget::new(self.cores, self.share_cap());
            }
            self.share_step(&mut g, now_ms())
        };
        for id in changed {
            self.publish(id);
        }
        Ok(self.admission_view())
    }

    /// The busy CPUs of the pool and the compile cache over the last
    /// `n` samples, as admission by share reads them.
    fn cpu_figure_last(&self, g: &Inner, now: i64, n: usize) -> cpu_limit::Figure {
        if let Some(read) = self.cfg.cpu_busy {
            return read().map_or(cpu_limit::Figure::Unknown, cpu_limit::Figure::Busy);
        }
        match g.host_busy.mean_last(now, n) {
            Some(parts) => cpu_limit::Figure::Busy(parts),
            None => cpu_limit::Figure::Unknown,
        }
    }

    /// Admission by share, once a tick and after a change of its settings.
    ///
    /// By share: moves the budget, computes the free share from the census
    /// and the jobs ([`crate::shares::room`]), hands it to the pool, sets
    /// the tokens in circulation to the budget and tells each job queued
    /// in the pool why it waits; a running job's used share and what it
    /// lent go into its record. By slots: hands the queue back to the
    /// pool's tokens once. Called under the server lock (`g`). Returns the
    /// jobs whose record changed.
    fn share_step(&self, g: &mut Inner, now: i64) -> Vec<i64> {
        use crate::shares::{self, Mode};
        let settings = *self.shares.lock().expect("shares lock");
        let mut changed = Vec::new();
        if settings.mode == Mode::Slots {
            // A job that started by share holds no slot. Until it ends one
            // token for it stays held back, so the pool does not run its
            // whole FIFO beside it.
            let owed = g
                .active
                .values()
                .filter(|l| l.started() && l.held == 0 && l.job.admission == Mode::Shares)
                .count();
            self.pool.set_owed(owed);
            if self.pool.shares().is_some() {
                self.pool.set_shares(None);
                self.pool
                    .set_limit(self.circulating(self.headroom_percent()));
                for (id, l) in &mut g.active {
                    // A job in the pool's queue has no reason of its own
                    // by slots.
                    let queued = l.waiter.is_some() && !l.job.wait_reason.is_empty();
                    if queued {
                        l.job.wait_reason.clear();
                    }
                    // A job that started by share keeps its share: it is
                    // counted and charged by share until it ends.
                    let by_share = l.started() && l.job.admission == Mode::Shares;
                    let dropped = !by_share && l.job.share.take().is_some();
                    if dropped {
                        l.job.share_used = None;
                    }
                    if dropped || queued {
                        changed.push(*id);
                    }
                }
            }
            g.share_view = settings.view(self.cores);
            g.share_last_ms = None;
            return changed;
        }
        self.pool.set_owed(0);
        let cap = self.share_cap();
        // The budget falls no lower than the setting, nor than the CPUs
        // the floor's jobs (the oldest running ones) asked for.
        let mut started: Vec<(i64, f64)> = g
            .active
            .iter()
            .filter(|(_, l)| l.started())
            .map(|(id, l)| (*id, l.share))
            .collect();
        started.sort_by_key(|s| s.0);
        let floor_cpus: f64 = started.iter().take(settings.floor()).map(|s| s.1).sum();
        let lower = settings.min_budget(self.cores).max(floor_cpus);
        let budget = g.budget.step(now, cap, lower);
        // Without a figure (the first look after a start or a reload, a
        // sampler late by more than its staleness) every running job
        // counts with its whole share, as one that just started.
        let (busy, known) = match self.cpu_figure_last(g, now, shares::WINDOW_S) {
            cpu_limit::Figure::Busy(p) => (p.counted(), true),
            cpu_limit::Figure::Pending | cpu_limit::Figure::Unknown => (0.0, false),
        };
        let dt = g
            .share_last_ms
            .map_or(0.0, |t| (now - t).max(0) as f64 / 1000.0);
        g.share_last_ms = Some(now);
        let mut jobs = Vec::new();
        for (id, l) in &mut g.active {
            let share = round2(l.share);
            if l.job.share != Some(share) {
                l.job.share = Some(share);
                changed.push(*id);
            }
            if l.started() && !known {
                jobs.push(shares::Running {
                    share: l.share,
                    shown: 0.0,
                    young: true,
                    idle: false,
                });
            } else if l.started() {
                let shown = l.recent.share(shares::WINDOW_S);
                let idle =
                    l.pid.is_some() && l.recent.idle(shares::IDLE_SAMPLES, shares::IDLE_CORES);
                let young = l.pid.is_none() || now - l.job.started_ms < shares::START_MS;
                if l.pid.is_some() {
                    l.job.share_used = Some(round2(shown));
                }
                if idle {
                    l.job.lent_s += (l.share - shown).max(0.0) * dt;
                }
                jobs.push(shares::Running {
                    share: l.share,
                    shown,
                    young,
                    idle,
                });
            } else if l.waiter.as_ref().is_some_and(WaiterHandle::granted) {
                // Let through by the pool, not claimed yet.
                jobs.push(shares::Running {
                    share: l.share,
                    shown: 0.0,
                    young: true,
                    idle: false,
                });
            }
        }
        let room = shares::room(budget, busy, &jobs);
        let pressure = self.cpu_pressure(g);
        let braking = pressure > settings.pressure_max();
        self.pool.set_shares(Some(jobserver::ShareRoom {
            free: room.free,
            running: jobs.len(),
            floor: settings.floor(),
            max: settings.max(),
            brake: braking,
        }));
        // The FIFO's tokens bound the build processes inside the jobs:
        // as many as the budget has CPUs.
        let limit = limits::effective_tokens(self.cfg.tokens, self.headroom_percent())
            .min(budget_tokens(budget));
        if self.pool.limit() != limit {
            self.pool.set_limit(limit);
        }
        let left = self.pool.shares().unwrap_or_default();
        g.share_view = shares::View {
            admission: Mode::Shares,
            floor: settings.floor(),
            max: settings.max(),
            pressure_max: settings.pressure_max(),
            cpus: self.cores,
            budget,
            cap,
            min_budget: lower.min(cap),
            free: room.free,
            busy,
            starting: room.starting,
            running: left.running,
            idle: room.idle,
            lent: room.lent,
            pressure,
            braking,
        };
        for (id, l) in &mut g.active {
            let Some(w) = &l.waiter else { continue };
            if w.granted() {
                continue;
            }
            let reason = share_wait(
                l.share,
                predicted_short(l.job.predicted_ms),
                &left,
                &g.share_view,
            );
            if l.job.wait_reason != reason {
                l.job.wait_reason = reason;
                changed.push(*id);
            }
        }
        changed.sort_unstable();
        changed.dedup();
        changed
    }

    /// CPUs a job of `repo` that runs `argv` is expected to use: what its
    /// earlier runs used, at most the host's CPUs; 1 without history.
    fn expected_cores(&self, history: &admission::History, repo: &str, argv: &[String]) -> f64 {
        history
            .expected_cores(repo, argv)
            .unwrap_or(cpu_limit::DEFAULT_CORES)
            .min(f64::from(self.cores))
    }

    /// The host's busy CPUs as the CPU limit reads them: the mean of the
    /// census sampler's last samples, in the parts pool, compile cache and
    /// outside ([`census::Sample::parts`]). No scan of its own.
    fn cpu_figure(&self, g: &Inner, now: i64) -> cpu_limit::Figure {
        if let Some(read) = self.cfg.cpu_busy {
            return read().map_or(cpu_limit::Figure::Unknown, cpu_limit::Figure::Busy);
        }
        match g.host_busy.mean(now) {
            Some(parts) => cpu_limit::Figure::Busy(parts),
            None if now - self.born_ms < cpu_limit::GRACE_MS => cpu_limit::Figure::Pending,
            None => cpu_limit::Figure::Unknown,
        }
    }

    /// Whether the CPU limit holds queued job `id` now. Called under the
    /// server lock (`g`), so the jobs that claimed their slots a moment ago
    /// are counted.
    fn cpu_verdict(
        &self,
        g: &Inner,
        limits: cpu_limit::Limits,
        id: i64,
        now: i64,
    ) -> cpu_limit::Verdict {
        if limits.cpu_busy_max.is_none() {
            return cpu_limit::Verdict::Free;
        }
        let Some(me) = g.active.get(&id) else {
            return cpu_limit::Verdict::Free;
        };
        let mut load = cpu_limit::Load {
            figure: self.cpu_figure(g, now),
            cpus: self.cores,
            running: 0,
            starting: 0.0,
            ahead: 0.0,
            held_ms: me.cpu_wait_since.map(|since| now - since),
            pressure: self.cpu_pressure(g),
        };
        let window = g.host_busy.len();
        let mine = (me.job.priority, id);
        for (other, l) in &g.active {
            if *other == id {
                continue;
            }
            if l.started() {
                load.running += 1;
                if l.pid.is_none() || now - l.job.started_ms < cpu_limit::STARTING_MS {
                    load.starting += (l.expect_cores - l.recent.share(window)).max(0.0);
                }
            } else if l.cpu_wait && (l.job.priority, *other) < mine {
                load.ahead += l.expect_cores;
            }
        }
        let v = cpu_limit::verdict(limits, &load, self.cfg.cpu_hold_max);
        // A job predicted short is not held: it would add little load for
        // a short time.
        if v != cpu_limit::Verdict::Free
            && let Some(ms) = me.job.predicted_ms.filter(|&ms| ms <= cpu_limit::PASS_MS)
        {
            return cpu_limit::Verdict::Short(ms);
        }
        // Jobs that reach the bound together start one by one.
        let gap = i64::try_from(self.cfg.cpu_bound_gap.as_millis()).unwrap_or(i64::MAX);
        if v == cpu_limit::Verdict::Overdue && now - g.cpu_bound_ms < gap {
            return cpu_limit::Verdict::Held(format!(
                "host busy: held as long as a job is; one such job starts every {} s",
                (gap + 999) / 1000
            ));
        }
        v
    }

    /// Job `id` starts although the host is busier than the mark: it was
    /// held for the host's CPUs as long as a job is ([`Config::cpu_hold_max`]).
    fn cpu_hold_ran_out(&self, id: i64, tx: &mpsc::UnboundedSender<Frame>) {
        let now = now_ms();
        let held_ms = {
            let g = self.inner.lock().expect("server lock");
            g.active
                .get(&id)
                .and_then(|l| l.cpu_wait_since)
                .map_or(0, |since| now - since)
        };
        let msg = format!(
            "slot: job {id} was held {} for the host's CPUs, as long as a job is: it starts",
            run_limit::text(u64::try_from(held_ms / 1000).unwrap_or(0))
        );
        eprintln!("{msg}");
        let _ = tx.send(Frame::Info { msg });
        self.census_event(&serde_json::json!({
            "t": now,
            "event": "cpu_hold_bound",
            "job": id,
            "held_ms": held_ms,
        }));
    }

    /// Job `id` starts although the host is busier than the mark: its
    /// command is predicted to run `predicted_ms`, at most
    /// [`cpu_limit::PASS_MS`].
    fn cpu_hold_passed(&self, id: i64, predicted_ms: u64, tx: &mpsc::UnboundedSender<Frame>) {
        let msg = format!(
            "slot: job {id} is predicted to run {} s: it is not held for the host's CPUs",
            predicted_ms.div_ceil(1000)
        );
        let _ = tx.send(Frame::Info { msg });
        self.census_event(&serde_json::json!({
            "t": now_ms(),
            "event": "cpu_hold_short",
            "job": id,
            "predicted_ms": predicted_ms,
        }));
    }

    /// Marks every running job that ran past its limit as timed out and
    /// tells its supervisor, which ends it as `slot cancel` would: SIGTERM
    /// to the process group, SIGKILL 5 s later.
    fn end_overdue(&self) {
        let now = now_ms();
        let mut g = self.inner.lock().expect("server lock");
        if g.handing_over {
            return;
        }
        for l in g.active.values_mut() {
            let Some(limit) = l.job.limit_s else { continue };
            // Not a job that ended or is being cancelled already.
            if l.pid.is_none()
                || l.exited.is_some()
                || *l.overdue.borrow()
                || l.stop.as_ref().is_some_and(|s| *s.gone.borrow())
            {
                continue;
            }
            let limit_ms = i64::try_from(limit.saturating_mul(1000)).unwrap_or(i64::MAX);
            if now - l.job.started_ms >= limit_ms {
                l.job.timed_out = true;
                l.overdue.send_replace(true);
            }
        }
    }

    /// Every second: ends the running jobs that ran past their run-time
    /// limit.
    async fn run_limit_watch(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            self.end_overdue();
        }
    }

    /// Every 15 s, writes back jobserver tokens that a process took and
    /// never returned (a compiler killed by a cancel or a `timeout`), and
    /// takes out tokens beyond the pool size. The idle-time reconcile
    /// cannot do either on a pool that is never idle.
    ///
    /// The rule (`docs/design.md` § 21): never write back a token a live
    /// process may hold. See [`Server::token_pass`].
    async fn token_watch(self: Arc<Self>) {
        let mut tick = tokio::time::interval(self.cfg.token_watch);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut memo = TokenMemo::default();
        loop {
            tick.tick().await;
            self.token_pass(&mut memo).await;
            self.watch_passes.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Slots granted to running jobs, and with them the tokens the pool
    /// cannot place and those beyond its size. Read under the server lock,
    /// so a job that ends is counted either with its slots or without.
    fn token_count(&self) -> (usize, usize, usize) {
        let g = self.inner.lock().expect("server lock");
        let granted: usize = g.active.values().map(|l| l.held).sum();
        (
            granted,
            self.pool.missing(granted),
            self.pool.surplus(granted),
        )
    }

    /// Records the tokens that neither a job nor a live holder accounts
    /// for, and since when there are any.
    fn set_unaccounted(&self, n: usize) {
        self.unaccounted.store(n as u64, Ordering::Relaxed);
        if n == 0 {
            self.unaccounted_since.store(0, Ordering::Relaxed);
        } else {
            let _ = self.unaccounted_since.compare_exchange(
                0,
                now_ms(),
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        }
    }

    fn census_event(&self, line: &serde_json::Value) {
        let mut g = self.inner.lock().expect("server lock");
        if let Ok(mut b) = serde_json::to_vec(line) {
            b.push(b'\n');
            let _ = g.census_log.write_all(&b);
        }
    }

    /// One pass of [`Server::token_watch`].
    ///
    /// Lost tokens: the tokens the pool cannot place (not in the FIFO, not
    /// held back, not granted to a job, not gathered by a queued request)
    /// are with live processes that have the FIFO open, or lost. Each live
    /// holder counts for the most it can hold ([`jobserver::Holder::may_hold`]:
    /// an upper bound, so a slow holder and a holder whose children are
    /// clients of a compile server keep theirs); what is missing beyond
    /// that sum is the shortfall. The unplaced count is read before and
    /// after the walk over `/proc` and the smaller one counts, so a token
    /// given back meanwhile is not missing. The pass writes back the
    /// smaller of two shortfalls of passes in a row, so a token the reader
    /// thread is booking at that moment is not taken for lost.
    ///
    /// Surplus tokens: more tokens placed than the pool has, whatever live
    /// holders hold besides. The pass takes out the smaller of two such
    /// counts in a row.
    async fn token_pass(&self, memo: &mut TokenMemo) {
        let (granted, missing, surplus) = self.token_count();
        let take = memo.surplus.map_or(0, |prev| prev.min(surplus));
        memo.surplus = (surplus > 0 && take == 0).then_some(surplus);
        if take > 0 {
            self.pool.remove(take);
            self.removed.fetch_add(take as u64, Ordering::Relaxed);
            eprintln!(
                "slot: {take} jobserver token(s) more than the pool's {} were in circulation \
                 ({granted} granted to jobs): took them out",
                self.cfg.tokens
            );
            self.census_event(&serde_json::json!({
                "t": now_ms(),
                "event": "tokens_removed",
                "tokens": take,
                "granted": granted,
            }));
        }
        if missing == 0 {
            memo.short = None;
            self.set_unaccounted(0);
            return;
        }
        let path = self.pool.path().to_path_buf();
        let Ok(Some(holders)) =
            tokio::task::spawn_blocking(move || jobserver::holders(&path, std::process::id()))
                .await
        else {
            // Nothing is known about who holds tokens: no count, no action.
            memo.short = None;
            return;
        };
        let bound: usize = holders.iter().map(jobserver::Holder::may_hold).sum();
        let (granted, after, _) = self.token_count();
        let short = missing.min(after).saturating_sub(bound);
        let n = memo.short.map_or(0, |prev| prev.min(short));
        memo.short = (short > 0 && n == 0).then_some(short);
        self.set_unaccounted(short - n);
        if n == 0 {
            return;
        }
        self.pool.restore(n);
        self.restored.fetch_add(n as u64, Ordering::Relaxed);
        eprintln!(
            "slot: {n} jobserver token(s) were lost ({granted} granted to jobs, {} live \
             process(es) with the FIFO open hold at most {bound}, {} waiting): wrote them back",
            holders.len(),
            self.pool.waiting()
        );
        self.census_event(&serde_json::json!({
            "t": now_ms(),
            "event": "tokens_restored",
            "tokens": n,
            "granted": granted,
            "holders": holders.len(),
            "bound": bound,
        }));
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

    /// Once a minute, appends the counters of every running sccache server
    /// of this user to `cache.jsonl` in the state directory, one line each
    /// ([`crate::sccache::StatsLog`]). The file only grows, like the census
    /// log. A server that cannot be read is named once in the journal and
    /// skipped.
    async fn cache_stats_watch(self: Arc<Self>) {
        let path = self.cfg.state_dir.join("cache.jsonl");
        let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!(
                    "slot: {}: {e}; sccache counters are not recorded",
                    path.display()
                );
                return;
            }
        };
        let log = Arc::new(Mutex::new(crate::sccache::StatsLog::continuing(&path)));
        let mut tick = tokio::time::interval(CACHE_STATS_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let l = Arc::clone(&log);
            let Ok(pass) = tokio::task::spawn_blocking(move || {
                let snap = Snapshot::scan();
                l.lock().expect("stats lock").pass(&snap, now_ms())
            })
            .await
            else {
                continue;
            };
            for e in &pass.errors {
                eprintln!("{e}");
            }
            let mut lines = Vec::new();
            for r in &pass.records {
                if let Ok(b) = serde_json::to_vec(r) {
                    lines.extend(b);
                    lines.push(b'\n');
                }
            }
            if !lines.is_empty() {
                let _ = file.write_all(&lines);
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
    /// has less free space than the mark, deletes the least recently built
    /// ones until it has the mark plus the margin; in a dry run it only
    /// says which. The mark is the larger of `--evict-below` (or the mark
    /// `slot evict-below` set, read at every look) and `--min-free-disk`
    /// ([`crate::evict::mark`]), so eviction runs whenever the disk hold
    /// holds builds. A job held for disk wakes the watch at once when its
    /// last look found enough free space or deleted something; a look that
    /// found too little and could delete nothing waits for the minute. A
    /// new mark always wakes it. See [`crate::evict`].
    async fn evict_watch(self: Arc<Self>) {
        let flag = self.cfg.evict.dry_run;
        let disk_free = self.cfg.disk_free.unwrap_or(admission::disk_free);
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_said = Vec::new();
        let mut wake = true;
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                () = self.evict_wake.notified(), if wake => {}
                () = self.evict_set.notified() => {}
            }
            let below = crate::evict::below(
                *self.evict_below.lock().expect("evict below lock"),
                self.cfg.evict.floor,
            );
            let floor = crate::evict::mark(below, self.cfg.admission.min_free_disk);
            let raised_from = if floor > below { below } else { 0 };
            if floor == 0 {
                let mut r = self.evict.lock().expect("evict lock");
                r.floor = 0;
                r.raised_from = 0;
                wake = true;
                continue;
            }
            let s = crate::evict::Settings {
                floor,
                dry_run: crate::evict::dry_run(&self.cfg.state_dir, flag),
                min_idle: crate::evict::min_idle(
                    *self.evict_idle.lock().expect("evict idle lock"),
                    self.cfg.evict.min_idle,
                ),
                ..self.cfg.evict
            };
            // A directory whose removal failed in the last hour is left
            // out like a busy one, so the pass plans the next ones.
            let failed: Vec<PathBuf> = {
                let keep = i64::try_from(crate::evict::FAILED_KEEP.as_millis()).unwrap_or(i64::MAX);
                let now = now_ms();
                let mut f = self.evict_failed.lock().expect("evict failed lock");
                f.retain(|_, at| now - *at < keep);
                f.keys().cloned().collect()
            };
            let (seen, busy_dirs, busy_cwds) = {
                let g = self.inner.lock().expect("server lock");
                let dirs: HashSet<PathBuf> = g
                    .active
                    .values()
                    .filter(|l| !l.job.target_dir.is_empty())
                    .map(|l| PathBuf::from(&l.job.target_dir))
                    .chain(g.active.values().filter_map(|l| l.env_target.clone()))
                    .chain(failed)
                    .collect();
                // The trees of queued and running jobs, and of every job
                // that ran in the last hour: the steps of one long task
                // keep their target directories between them.
                let cwds: HashSet<PathBuf> = g
                    .active
                    .values()
                    .filter(|l| !l.job.cwd.is_empty())
                    .map(|l| PathBuf::from(&l.job.cwd))
                    .chain(crate::evict::ran_recently(&g.ran_in, now_ms()))
                    .collect();
                (g.targets.clone(), dirs, cwds)
            };
            let known = seen.len() as u32;
            let Ok((lowest, planned)) = tokio::task::spawn_blocking(move || {
                evict_pass_with(&seen, &busy_dirs, &busy_cwds, now_ms(), s, disk_free)
            })
            .await
            else {
                continue;
            };
            let free = lowest.unwrap_or(0);
            let mut done = Vec::new();
            for (c, bytes) in planned {
                let action = if s.dry_run {
                    "would delete".to_string()
                } else {
                    self.evict_one(&c).await
                };
                if action.starts_with("failed") {
                    eprintln!(
                        "slot: disk: eviction leaves {} out for {}: its removal failed",
                        c.path.display(),
                        crate::run_limit::text(crate::evict::FAILED_KEEP.as_secs())
                    );
                    self.evict_failed
                        .lock()
                        .expect("evict failed lock")
                        .insert(c.path.clone(), now_ms());
                }
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
                if s.dry_run {
                    eprintln!(
                        "slot: {}",
                        crate::evict::off_summary(free, s.floor, raised_from, &done)
                    );
                }
                for e in &done {
                    eprintln!(
                        "slot: disk: {} free, under {}: {} {} ({}, last built {} ago)",
                        admission::gb(free),
                        crate::evict::mark_text(s.floor, raised_from),
                        e.action,
                        e.path,
                        admission::gb(e.bytes),
                        crate::status::secs(now_ms() - e.last_ms)
                    );
                }
            }
            last_said = said;
            let freed: u64 = done
                .iter()
                .filter(|e| e.action == "deleted" || e.action == "discard-cache")
                .map(|e| e.bytes)
                .sum();
            wake = lowest.is_none_or(|f| f >= s.floor) || freed > 0;
            let mut r = self.evict.lock().expect("evict lock");
            r.floor = s.floor;
            r.raised_from = raised_from;
            r.known = known;
            r.dry_run = s.dry_run;
            r.free = free;
            r.checked_ms = now_ms();
            if !done.is_empty() || free >= s.floor {
                r.deleted_bytes += freed;
                r.last = done;
            }
        }
    }

    /// Deletes one target directory unless a job that builds into it or
    /// runs in its checkout started meanwhile; returns what happened.
    async fn evict_one(&self, c: &crate::evict::Candidate) -> String {
        {
            let g = self.inner.lock().expect("server lock");
            let busy = g.active.values().any(|l| {
                crate::evict::job_keeps(
                    &c.path,
                    Path::new(&l.job.target_dir),
                    Path::new(&l.job.cwd),
                ) || l
                    .env_target
                    .as_deref()
                    .is_some_and(|t| crate::evict::holds_target(&c.path, t))
            }) || crate::evict::ran_recently(&g.ran_in, now_ms())
                .iter()
                .any(|cwd| crate::evict::job_keeps(&c.path, Path::new(""), cwd));
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
    /// `persist`, and sets `CPUQuota` on the daemon's unit when there is one:
    /// `(100 − h) × cores` percent on the pool base, the target less the
    /// smoothed CPU used outside the pool on the machine base.
    async fn apply_headroom(self: &Arc<Self>, percent: u8, persist: bool) -> limits::View {
        let percent = percent.min(limits::MAX_HEADROOM);
        self.pool.set_limit(self.circulating(percent));
        if persist {
            self.headroom.lock().expect("headroom lock").saved_percent = percent;
            self.save_limits();
        }
        let _serial = self.quota_lock.lock().await;
        let base = self.headroom.lock().expect("headroom lock").base;
        let machine_q = (base == HeadroomBase::Machine && percent > 0).then(|| {
            let outside = self
                .inner
                .lock()
                .expect("server lock")
                .machine
                .outside()
                .unwrap_or(0.0);
            machine::quota(machine::target_cores(self.cores, percent), outside).cores
        });
        let quota = match machine_q {
            Some(q) => Some(machine::quota_percent(q)),
            None => limits::cpu_quota_percent(self.cores, percent),
        };
        let applied = self.set_quota(quota).await;
        {
            let mut h = self.headroom.lock().expect("headroom lock");
            h.percent = percent;
            h.machine_applied = machine_q.map(|q| (q, Instant::now()));
            h.record_quota(quota, applied);
        }
        self.limits()
    }

    /// Sets or clears `CPUQuota` on the daemon's unit, and on [`JOB_SLICE`]
    /// when jobs run there.
    async fn set_quota(&self, quota: Option<u32>) -> Result<(), String> {
        if !self.cfg.cpu_quota {
            return Err("cpu quota disabled (--cpu-quota false)".into());
        }
        let slice = self.cfg.memory_max.is_some();
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
    }

    /// Machine base, each tick: sets the quota `q` CPUs when it moved by
    /// [`machine::MIN_STEP_CORES`] and [`machine::MIN_APPLY_GAP`] passed since
    /// the last one, so `systemctl` runs at most every 10 s.
    fn plan_machine_quota(self: &Arc<Self>, q: f64) {
        if !self.cfg.cpu_quota {
            return;
        }
        let now = Instant::now();
        {
            let mut h = self.headroom.lock().expect("headroom lock");
            if h.base != HeadroomBase::Machine
                || h.percent == 0
                || !machine::should_apply(h.machine_applied, q, now)
            {
                return;
            }
            h.machine_applied = Some((q, now));
        }
        let s = Arc::clone(self);
        tokio::spawn(async move {
            let _serial = s.quota_lock.lock().await;
            {
                // The base or the headroom may have changed meanwhile.
                let h = s.headroom.lock().expect("headroom lock");
                if h.base != HeadroomBase::Machine || h.percent == 0 {
                    return;
                }
            }
            let quota = Some(machine::quota_percent(q));
            let applied = s.set_quota(quota).await;
            s.headroom
                .lock()
                .expect("headroom lock")
                .record_quota(quota, applied);
        });
    }

    /// The `GET /api/state` snapshot of the local pool.
    fn local_state(&self) -> State {
        let g = self.inner.lock().expect("server lock");
        let m = &g.machine_shown;
        // A guard taken inside the struct literal would live until its end,
        // and the calls below lock the headroom again.
        let base = self.headroom.lock().expect("headroom lock").base;
        let run_limit = self.run_limit_view();
        let job_parallel = self.job_parallel_view().job_parallel;
        let cpu = self.cpu_limits();
        State {
            run_limit_s: run_limit.run_limit_s,
            run_limit_max_s: Some(run_limit.run_limit_max_s),
            job_parallel,
            cpu_busy_max: cpu.cpu_busy_max,
            cpu_busy_floor: cpu.cpu_busy_max.map(|_| cpu.floor()),
            cpu_pressure_max: cpu.cpu_pressure_max,
            headroom_base: base,
            busy_cores: m.busy,
            host_busy_cores: Some(g.sys_cpu * f64::from(self.cores)),
            load1: procstat::load1(),
            target_cores: m.target,
            outside_cores: m.outside,
            outside: m.owners.clone(),
            quota_floor: m.floor,
            over_target: m.over,
            host: self.host.clone(),
            cores: self.cores,
            tokens: self.cfg.tokens as u32,
            limit: self.pool.limit() as u32,
            headroom_percent: u32::from(self.headroom_percent()),
            call_apps: self.call_apps(),
            cpu_quota_percent: self.headroom.lock().expect("headroom lock").quota,
            tokens_unaccounted: self.unaccounted.load(Ordering::Relaxed) as u32,
            tokens_unaccounted_since_ms: self.unaccounted_since.load(Ordering::Relaxed),
            shares: (self.shares_on() && g.share_view.admission == crate::shares::Mode::Shares)
                .then_some(g.share_view),
            free: self.pool.free() as u32,
            active: g
                .active
                .values()
                .map(|l| {
                    let mut j = l.job.clone();
                    if j.state == JobState::Queued {
                        (j.collected, j.ahead, j.queue_class) = Self::queue_marks(l);
                    }
                    on_local_node(j)
                })
                .collect(),
            done: g
                .done
                .iter()
                .map(|d| on_local_node(d.job.clone()))
                .collect(),
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

    /// The daemon's own machine as a node of `GET /api/nodes`.
    fn local_node(&self) -> Node {
        let now = now_ms();
        let g = self.inner.lock().expect("server lock");
        let cpu_busy = match self.cpu_figure(&g, now) {
            cpu_limit::Figure::Busy(p) => p.counted(),
            _ => 0.0,
        };
        let jobs: Vec<&Job> = g.active.values().map(|l| &l.job).collect();
        let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let disk_free = self.cfg.disk_free.unwrap_or(admission::disk_free);
        // Eviction's last look; before one, the filesystem of the state.
        let evicted = self.evict.lock().expect("evict lock").free;
        local_node(&LocalFacts {
            cluster: &self.host,
            cpus: self.cores,
            slots: self.pool.limit() as u32,
            memory: admission::parse_meminfo_total(&meminfo).unwrap_or(0),
            available: admission::parse_meminfo(&meminfo).unwrap_or(0),
            disk_total: admission::disk_total(&self.cfg.state_dir).unwrap_or(0),
            disk_free: if evicted > 0 {
                evicted
            } else {
                disk_free(&self.cfg.state_dir).unwrap_or(0)
            },
            joined_ms: self.born_ms,
            cpu_busy,
            jobs: &jobs,
            host: HostFacts {
                booted_ms: procstat::boot_time_ms(),
                poweroff_ms: procstat::scheduled_poweroff_ms(),
                busy: g.sys_cpu * f64::from(self.cores),
                load1: procstat::load1(),
            },
            ended: &g.tally,
            now,
        })
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

/// A job as the API shows it: one of a job-log line written before nodes
/// ran on the local node.
fn on_local_node(mut j: Job) -> Job {
    if j.node.is_empty() {
        j.node = LOCAL_NODE.to_string();
    }
    j
}

/// What the daemon knows of its own machine, read for `GET /api/nodes`.
struct LocalFacts<'a> {
    cluster: &'a str,
    cpus: u32,
    slots: u32,
    /// `MemTotal`, bytes.
    memory: u64,
    /// `MemAvailable`, bytes.
    available: u64,
    disk_total: u64,
    disk_free: u64,
    joined_ms: i64,
    cpu_busy: f64,
    /// Queued and running jobs.
    jobs: &'a [&'a Job],
    host: HostFacts,
    /// The jobs this node ended.
    ended: &'a Tally,
    now: i64,
}

/// What the daemon reads of the whole machine, beyond its pool.
struct HostFacts {
    /// Boot time, Unix ms.
    booted_ms: Option<i64>,
    /// The poweroff systemd has scheduled, Unix ms.
    poweroff_ms: Option<i64>,
    /// CPUs busy on the whole machine over the last sample.
    busy: f64,
    /// One-minute load average.
    load1: Option<f64>,
}

const GIB: u64 = 1 << 30;

/// Bytes in whole GiB, rounded, as the API's sizes are given.
fn whole_gib(b: u64) -> u32 {
    u32::try_from((b + GIB / 2) / GIB).unwrap_or(u32::MAX)
}

/// The daemon's own machine as a node.
fn local_node(f: &LocalFacts<'_>) -> Node {
    let running = || f.jobs.iter().filter(|j| j.state == JobState::Running);
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    Node {
        node_id: LOCAL_NODE.to_string(),
        cluster_id: f.cluster.to_string(),
        kind: NodeKind::Local,
        state: NodeState::Ready,
        cpus: f.cpus,
        slots: f.slots,
        slots_in_use: running().map(|j| j.tokens).sum::<u32>().min(f.slots),
        memory_gib: whole_gib(f.memory),
        disk_gib: whole_gib(f.disk_total),
        max_worktrees: None,
        running_jobs: count(running().count()),
        queued_jobs: count(
            f.jobs
                .iter()
                .filter(|j| j.state == JobState::Queued)
                .count(),
        ),
        slot_version: env!("CARGO_PKG_VERSION").to_string(),
        cpu_busy_milli: Some((f.cpu_busy.max(0.0) * 1000.0).round() as u32),
        memory_used_mib: Some(
            u32::try_from(f.memory.saturating_sub(f.available) >> 20).unwrap_or(u32::MAX),
        ),
        disk_free_gib: Some(whole_gib(f.disk_free)),
        location: None,
        machine_type: None,
        price_per_hour_cents: None,
        joined_at: Some(crate::types::rfc3339(f.joined_ms)),
        deadline: None,
        gone_reason: None,
        left_at: None,
        booted_at: f.host.booted_ms.map(crate::types::rfc3339),
        poweroff_at: f.host.poweroff_ms.map(crate::types::rfc3339),
        host_cpu_milli: Some((f.host.busy.max(0.0) * 1000.0).round() as u32),
        load1: f.host.load1,
        ended_hour: Some(f.ended.last_hour(f.now)),
        ended_hours: Some(f.ended.hours(f.now)),
    }
}

/// Resolves when the client has gone away.
async fn client_gone(mut rx: watch::Receiver<bool>) {
    let _ = rx.wait_for(|g| *g).await.map(|_| ());
}

/// Resolves when the job ran past its run-time limit; never for a job that
/// left the table.
async fn past_limit(rx: Option<watch::Receiver<bool>>) {
    if let Some(mut rx) = rx
        && rx.wait_for(|o| *o).await.is_ok()
    {
        return;
    }
    std::future::pending::<()>().await;
}

/// `v` to two decimals, as job records carry shares.
fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Whether a job predicted to run `predicted_ms` is short: it starts ahead
/// of the longer ones of its level, and under the pressure brake.
fn predicted_short(predicted_ms: Option<u64>) -> bool {
    predicted_ms.is_some_and(|ms| ms <= priority::SHORT_MS)
}

/// Tokens in circulation for a budget of `budget` CPUs, at least 1.
fn budget_tokens(budget: f64) -> usize {
    (budget.round() as usize).max(1)
}

/// Why a job of `share` CPUs waits in the pool's queue by share, with
/// `left` the room after this look's grants and `v` its figures.
fn share_wait(
    share: f64,
    short: bool,
    left: &jobserver::ShareRoom,
    v: &crate::shares::View,
) -> String {
    if left.running >= left.max {
        return format!("{} jobs run, the most by share", left.max);
    }
    if v.braking && !short {
        return format!(
            "CPU pressure {:.0} % over {:.0} %: only short jobs start",
            v.pressure, v.pressure_max
        );
    }
    format!(
        "waits for a share of {share:.1} CPUs: {:.1} of a budget of {:.1} free",
        left.free.max(0.0),
        v.budget
    )
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
/// the socket takes them; a request on `ctl` gets every unsent byte, the
/// frames still in `rx` included, and the writer then writes nothing more,
/// for a handover to pass on. Ends after the exit frame, or when every
/// sender is gone, once `pending` is written.
async fn client_writer(
    mut wr: OwnedWriteHalf,
    mut rx: mpsc::UnboundedReceiver<Frame>,
    mut ctl: mpsc::UnboundedReceiver<oneshot::Sender<Vec<u8>>>,
    mut pending: Vec<u8>,
) {
    /// Appends `f` to `pending`; whether it was the exit frame.
    fn queue(pending: &mut Vec<u8>, f: &Frame) -> bool {
        if let Ok(b) = serde_json::to_vec(f) {
            pending.extend_from_slice(&b);
            pending.push(b'\n');
        }
        matches!(f, Frame::Exit { .. })
    }
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
                    // The handover stopped the pumps before it asked: what
                    // they sent is in the channel now, and unsent too.
                    while rx_open && !closing {
                        match rx.try_recv() {
                            Ok(f) => closing = queue(&mut pending, &f),
                            Err(_) => break,
                        }
                    }
                    let _ = reply.send(std::mem::take(&mut pending));
                    std::future::pending::<()>().await;
                }
                None => ctl_open = false,
            },
            f = rx.recv(), if rx_open && !closing => match f {
                Some(f) => closing = queue(&mut pending, &f),
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
    running: Vec<(handover::Running, Option<u64>, Arc<Ring>)>,
    queued: Vec<(handover::Queued, Option<u64>)>,
    pumps: Vec<Vec<std::thread::JoinHandle<PumpEnd>>>,
    next_id: i64,
    done_output: Vec<(i64, Arc<Ring>)>,
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
#[cfg(test)]
fn evict_pass(
    seen: &HashMap<PathBuf, (String, i64)>,
    busy_dirs: &HashSet<PathBuf>,
    busy_cwds: &HashSet<PathBuf>,
    now: i64,
    s: crate::evict::Settings,
) -> (Option<u64>, Vec<(crate::evict::Candidate, u64)>) {
    evict_pass_with(seen, busy_dirs, busy_cwds, now, s, admission::disk_free)
}

/// [`evict_pass`] with the free space read by `disk_free`
/// ([`Config::disk_free`]). The lowest free space is `None` when no known
/// target directory exists. `busy_dirs` and `busy_cwds` are the target
/// and working directories of queued and running jobs
/// ([`crate::evict::candidates`]); `busy_dirs` also holds the target
/// directories whose removal failed in the last hour.
fn evict_pass_with(
    seen: &HashMap<PathBuf, (String, i64)>,
    busy_dirs: &HashSet<PathBuf>,
    busy_cwds: &HashSet<PathBuf>,
    now: i64,
    s: crate::evict::Settings,
    disk_free: fn(&Path) -> Option<u64>,
) -> (Option<u64>, Vec<(crate::evict::Candidate, u64)>) {
    use crate::evict;
    let free = seen
        .keys()
        .filter(|p| p.exists())
        .filter_map(|p| disk_free(p))
        .min();
    let cands: Vec<evict::Candidate> =
        evict::candidates(seen, busy_dirs, busy_cwds, now, s.min_idle)
            .into_iter()
            .filter(|c| evict::is_target_dir(&c.path))
            .collect();
    let mut out = Vec::new();
    let mut in_use: Option<evict::InUse> = None;
    for group in evict::by_device(cands).into_values() {
        let Some(fs_free) = group.first().and_then(|c| disk_free(&c.path)) else {
            continue;
        };
        if fs_free >= s.floor {
            continue;
        }
        // A directory a process or a service uses is kept.
        let used = in_use.get_or_insert_with(evict::InUse::scan);
        let goal = s.floor.saturating_add(s.margin);
        let mut sized = Vec::new();
        let mut have = fs_free;
        for c in group {
            if have >= goal {
                break;
            }
            if let Some(why) = used.why(&c.path) {
                eprintln!("slot: disk: kept {}: {why}", c.path.display());
                continue;
            }
            let b = evict::dir_size(&c.path);
            have = have.saturating_add(b);
            sized.push((c, b));
        }
        out.extend(evict::plan(fs_free, s, &sized).into_iter().cloned());
    }
    (free, out)
}

/// The open-file limit job `req` runs with when it is not the daemon's
/// own: its client's, capped at the daemon's hard limit (one `slot:
/// warning:` on `tx` when it is capped). `None` for a request without one
/// (a remote request, an older client) and for a limit equal to the
/// daemon's: the job inherits the daemon's, which the daemon never changes,
/// so it is the limit the daemon started with, also after a reload.
fn job_file_limit(
    req: &Request,
    tx: &mpsc::UnboundedSender<Frame>,
) -> Option<crate::proto::FileLimit> {
    let want = req.job_nofile()?;
    let own = crate::proto::FileLimit::of_this_process();
    let (job, warning) = want.for_job(own.hard);
    if let Some(w) = warning {
        let _ = tx.send(Frame::Info {
            msg: format!("slot: warning: {w}"),
        });
    }
    (job != own).then_some(job)
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

/// Notes that `job` ran (or waited) in its working directory, with the
/// latest of its end, start and enqueue time.
fn record_ran(ran_in: &mut HashMap<PathBuf, i64>, job: &Job) {
    if job.cwd.is_empty() {
        return;
    }
    let when = job.ended_ms.max(job.started_ms).max(job.enqueued_ms);
    let e = ran_in.entry(PathBuf::from(&job.cwd)).or_insert(when);
    *e = (*e).max(when);
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

/// The time a CPU sample covers: from one `/proc` scan to the next.
///
/// The scan runs on a blocking thread after the second's tick. Under load
/// that thread starts late, so the time between two ticks is not the time
/// between two scans: CPU time over 1.9 s divided by 1 s read 37 CPUs on a
/// host of 20, and the next sample read too few. A scan that fails moves
/// nothing, so the next sample covers both seconds.
#[derive(Debug)]
struct ScanInterval(Instant);

impl ScanInterval {
    /// Seconds since the previous scan, which `at` replaces.
    fn next(&mut self, at: Instant) -> f64 {
        let dt = at.saturating_duration_since(self.0).as_secs_f64();
        self.0 = at;
        dt
    }
}

#[cfg(test)]
mod tests;
