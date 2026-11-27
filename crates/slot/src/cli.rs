//! The `slot` command line. Lives in the library so `slot-docs` can render
//! the reference from the same definition.

use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::paths;

/// Run builds on one shared pool of build slots.
///
/// `slot <command…>` sends the command to the slot daemon, which queues it
/// until enough slots are free, runs it, streams its output back and exits
/// with its code. Every build that goes through the daemon shares one
/// GNU-make jobserver, so cargo, rustc and make draw parallelism from the
/// same pool and the machine is never oversubscribed.
#[derive(Debug, Parser)]
#[command(name = "slot", version = env!("SLOT_VERSION"))]
pub struct Cli {
    /// Run the command here, without the daemon.
    #[arg(long, global = true)]
    pub local: bool,

    /// Keep the job in this host's pool, never on a build node: for a step
    /// that needs this host's credentials while it runs (a clone, a fetch,
    /// a push). `slot status` shows `local: here: needs this host` under it.
    #[arg(long, global = true)]
    pub here: bool,

    /// Slots the job holds, in place of the count slot derives from the
    /// command. For work the daemon cannot see, such as a script that
    /// starts `docker run --cpus 4`: `slot --tokens 4 ./build.sh`. The
    /// daemon caps it at the slots in circulation (the pool less the
    /// headroom), so the job never waits forever. Ignored with `--local`.
    #[arg(long, env = "SLOT_TOKENS", value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub tokens: Option<u32>,

    /// The job's queue level: queued `urgent` jobs get free slots first,
    /// then `high`, `normal` and `low` ones; with no slot free, an `urgent`
    /// job starts on one the headroom holds back. A rule set with `slot
    /// priority set` for the job's repository wins over it. Ignored with
    /// `--local`.
    #[arg(long, env = "SLOT_PRIORITY", value_name = "LEVEL", value_enum)]
    pub priority: Option<crate::types::Priority>,

    /// How long the job may run, counted from its start, in place of the
    /// daemon's run-time limit (`slot run-limit`): seconds, or a number
    /// with `s`, `m` or `h`, such as `90s`, `30m` or `1h`. The daemon ends
    /// a job that runs longer (SIGTERM, SIGKILL 5 s later) and the job
    /// exits 124. It is capped at the most a job may name, 1 h unless
    /// `slot run-limit --max` changed it. A `timeout` command in
    /// front of the job's command does not extend it. Ignored with
    /// `--local`.
    #[arg(long, env = "SLOT_TIMEOUT", value_name = "DURATION", value_parser = crate::run_limit::parse_duration)]
    pub timeout: Option<u64>,

    /// With `SLOT_HOST` set: after the job exits 0, copy PATH back from
    /// the build server into the checkout. PATH is relative to the
    /// repository root; `target/…` is read from the job's target directory
    /// on the server. Repeat for more paths. Without `SLOT_HOST` the
    /// output is already here and this does nothing.
    #[arg(long, global = true, value_name = "PATH")]
    pub fetch: Vec<String>,

    /// Subcommand, or the build command to run.
    #[command(subcommand)]
    pub command: Cmd,
}

/// Subcommands. Anything that is not one of them is a build command.
#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Run the daemon: the slot pool, the job queue and the web UI.
    Serve(Box<ServeArgs>),
    /// Bridge stdin/stdout to the daemon's socket (remote clients run this over ssh).
    Proxy(ProxyArgs),
    /// ssh forced command for a build server: runs only the sync, sibling,
    /// fetch, proxy and cancel steps named in `SSH_ORIGINAL_COMMAND`.
    Shell(ShellArgs),
    /// Serve gate directories: builds in a sandbox that can open FIFOs there,
    /// but cannot reach the daemon's socket, queue in the pool and run in
    /// their sandbox. The client sets `SLOT_GATE=<dir>`.
    Gate(GateArgs),
    /// The process the daemon runs for a gated job: holds its slots until
    /// the sandboxed client closes the FIFO. Started by the daemon only.
    #[command(hide = true)]
    Hold(HoldArgs),
    /// The process the daemon starts for a job whose open-file limit is not
    /// its own: sets the limit, then executes the job's program in its
    /// place (same pid, same process group). Started by the daemon only.
    #[command(name = "__exec", hide = true)]
    Exec(ExecArgs),
    /// The step a remote client runs on the build server before a sync:
    /// records the mirror's repository and prints the sibling mirrors it
    /// can copy from. Run by remote clients only.
    #[command(name = "__siblings", hide = true)]
    Siblings(SiblingsArgs),
    /// The step a remote client runs on the build server before a job: the
    /// server's cargo git database, and which of the named entries lack
    /// their revision. Run by remote clients only.
    #[command(name = "__cargo-git", hide = true)]
    CargoGit(CargoGitArgs),
    /// Run a build command; use this when the command's name is also a slot subcommand.
    Run(RunArgs),
    /// Show the pool and every queued and running job.
    Status(StatusArgs),
    /// Show or set the headroom: the share of the CPU kept free for you.
    Headroom(HeadroomArgs),
    /// Show or change the priority rules: the queue level of every job of a
    /// repository.
    #[command(subcommand)]
    Priority(PriorityCmd),
    /// Show or set the run-time limit: how long a job may run before the
    /// daemon ends it.
    RunLimit(RunLimitArgs),
    /// Show or set the limit on build processes per job: with it a job's
    /// compilers run on a bound of the job's own, not on slots of the pool.
    JobParallel(JobParallelArgs),
    /// Show or set the eviction mark: the free space below which the daemon
    /// deletes idle Cargo target directories, without a restart.
    EvictBelow(EvictBelowArgs),
    /// Show or set the CPU limit: the start of a queued job is held while
    /// the pool's jobs and the compile cache use more CPUs than a mark.
    CpuLimit(CpuLimitArgs),
    /// Show or set how queued jobs are admitted: by slots of the pool, or
    /// by a share of the CPUs while that share is free.
    Admission(AdmissionArgs),
    /// Cancel a queued or running job you submitted; a running job's
    /// processes get SIGTERM, then SIGKILL after 5 s.
    Cancel(CancelArgs),
    /// After a merge to `main`: build the tests of a checkout of `main` at
    /// the lowest level, so the next worktree of the repository starts from
    /// a built target directory.
    ///
    /// Run it in the checkout after it took the merge, for example from its
    /// `post-merge` hook. It returns at once; the daemon runs `cargo test
    /// --no-run`, or the `warm` command of the checkout's `.slot.toml`
    /// (`warm = false` switches it off). The daemon starts one only for a
    /// repository a job built on it in the last 24 hours, one at a time per
    /// repository, and cancels it when a job of a session waits for slots.
    Warm(WarmArgs),
    /// List agent sessions that build outside the pool, with their commands.
    Sessions(SessionsArgs),
    /// Show how the pool was shared: slot time, CPU time and queue wait per
    /// project, repository or session, from the job log.
    Usage(UsageArgs),
    /// The compile cache: what the sccache servers did, from the counters
    /// the daemon records once a minute.
    #[command(subcommand)]
    Cache(CacheCmd),
    /// The build nodes of this host: the `[remote.<name>]` entries of the
    /// host configuration file, and which node a repository's jobs go to.
    ///
    /// Any other `slot node …` command line (`slot node --test`, `slot node
    /// build.js`) is a build command that runs Node.js, as before.
    Node(NodeArgs),
    /// Agent hooks: refuse a build command that would run outside the pool.
    #[command(subcommand)]
    Hook(HookCmd),
    /// Any other command is run as a build: `slot cargo test -p foo`.
    #[command(external_subcommand)]
    External(Vec<OsString>),
}

/// `slot serve` options.
#[derive(Debug, Clone, Args)]
#[allow(clippy::struct_excessive_bools)]
pub struct ServeArgs {
    /// Unix socket clients connect to.
    #[arg(long, env = "SLOT_SOCKET", default_value_os_t = paths::default_socket())]
    pub socket: PathBuf,
    /// Web UI address; `off` disables it.
    #[arg(long, default_value = "127.0.0.1:7411")]
    pub http: String,
    /// Build slots in the pool (jobserver tokens); default: the number of CPUs.
    #[arg(long)]
    pub tokens: Option<usize>,
    /// Slots a go job holds; go's -p is set to match.
    #[arg(long, default_value_t = 4)]
    pub go_tokens: usize,
    /// Threads a test runner in a job may use when the caller sets none:
    /// `RUST_TEST_THREADS`, `NEXTEST_TEST_THREADS` and `GOMAXPROCS`
    /// (go jobs get their slot count). Default: a quarter of the pool, at least 2.
    /// Local jobs only: a job synced from another host gets its width, the
    /// free CPUs at its start split over the jobs that started in the last
    /// 5 s, and a lower count its caller set is raised to it.
    #[arg(long)]
    pub test_threads: Option<usize>,
    /// Whether a headroom also sets systemd `CPUQuota` on the daemon unit, a
    /// hard cap on every job together; `false` keeps only the token limit.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub cpu_quota: bool,
    /// Jobserver FIFO and job log.
    #[arg(long, default_value_os_t = paths::default_state_dir())]
    pub state_dir: PathBuf,
    /// Where remote clients sync workspaces (default: `<state-dir>/ws`).
    #[arg(long)]
    pub ws_root: Option<PathBuf>,
    /// Target dirs and caches for synced workspaces (default: `<state-dir>/cache`).
    #[arg(long)]
    pub cache_root: Option<PathBuf>,
    /// systemd `MemoryMax` per job, for example 24G.
    #[arg(long)]
    pub memory_max: Option<String>,
    /// Finished jobs kept in memory and shown in the UI.
    #[arg(long, default_value_t = 500)]
    pub keep_jobs: usize,
    /// systemd `CPUWeight` for every job (interactive apps default to 100);
    /// 0 leaves it unset. Applied to the daemon's own unit at start.
    #[arg(long, default_value_t = 20)]
    pub cpu_weight: u16,
    /// Call mode: headroom while a call uses the microphone or the camera,
    /// percent; 0 turns call mode off. `slot headroom --call` changes it at
    /// run time and that value is kept.
    #[arg(long, default_value_t = 40, value_parser = clap::value_parser!(u8).range(0..=50))]
    pub call_headroom: u8,
    /// Seconds without capture after which a call counts as ended and the
    /// headroom goes back.
    #[arg(long, default_value_t = 120)]
    pub call_off_after: u64,
    /// Applications whose capture is not a call (level meters, mixers),
    /// comma-separated; replaces the default list.
    #[arg(long, value_delimiter = ',', default_values_t = crate::call::DEFAULT_IGNORE.iter().map(|s| (*s).to_string()))]
    pub call_ignore: Vec<String>,
    /// Watch this user's sccache servers and write back the jobserver tokens
    /// a server lost, once it had none free for 60 s while compiles waited,
    /// and append each server's counters to `cache.jsonl` in the state
    /// directory once a minute; `false` turns both off.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub sccache_watch: bool,
    /// A job waits while the filesystem of its directory or of
    /// `CARGO_TARGET_DIR` has less free space than this, GB; 0 turns it off.
    /// Above `--evict-below`, eviction starts here instead.
    #[arg(long, default_value_t = 25.0, value_name = "GB")]
    pub min_free_disk: f64,
    /// The disk floor of a job whose last 5 runs of the same command in the
    /// same repository used no compile server and built into no target
    /// directory, GB; at most `--min-free-disk`, and no job starts below it.
    /// A first run waits at `--min-free-disk`. 0 lets such a job start on
    /// any free space. Default: the value of `--min-free-disk`.
    #[arg(long, value_name = "GB")]
    pub min_free_disk_light: Option<f64>,
    /// A job waits while available memory, less what jobs that started in
    /// the last 30 s still take, is below its expected peak (from earlier
    /// runs) plus this, GB; 0 turns it off. A job alone in the pool runs anyway.
    #[arg(long, default_value_t = 4.0, value_name = "GB")]
    pub min_free_memory: f64,
    /// After `systemctl --user reload slot`, the longest time no new job
    /// starts while jobs still run, seconds. Then jobs start again, and the
    /// next hold begins when the running jobs are expected to end within it
    /// (from earlier runs of the same command) or none runs. 0 never holds.
    #[arg(long, default_value_t = 60, value_name = "SECONDS")]
    pub upgrade_hold: u64,
    /// A queued `normal` or `low` job that has waited longer than this is
    /// ordered as `high`, so no level waits forever; 0 never promotes.
    /// `slot priority promote-after` changes it at run time; a value set
    /// that way wins over this option until `slot priority promote-after
    /// default` removes it.
    #[arg(long, default_value_t = 1800, value_name = "SECONDS")]
    pub promote_after: u64,
    /// A queued job may be passed by later jobs of its own level only until
    /// it has waited this long: by a job predicted to run 5 s or less, and by
    /// a job that fits in the slots a larger request collected and cannot use
    /// yet. From then on it starts before every job that came after it. A
    /// job of a lower level never passes it. 0: nobody passes; the queue is
    /// served by level, then arrival.
    #[arg(long, default_value_t = 300, value_name = "SECONDS")]
    pub pass_for: u64,
    /// For remote clients: GB the target directories of synced workspaces
    /// may take together. Before a remote job starts, the least recently
    /// used target directories and their source copies are deleted while
    /// the total is over this, or while the disk has less than
    /// `--min-free-disk` free; a workspace with a queued or running job is
    /// kept. 0: no budget, only the disk floor evicts.
    #[arg(long, default_value_t = 0.0, value_name = "GB")]
    pub cache_budget: f64,
    /// For remote clients: how the first job of a new worktree copies the
    /// target directory of another worktree of the same repository.
    /// `reflink`: only a reflink copy, none where the filesystem has no
    /// reflinks (a byte copy of a large target directory can take longer
    /// than the build it saves). `copy`: a reflink where the filesystem has
    /// them, else a byte copy. Never hard links.
    #[arg(long, value_enum, default_value_t = crate::server::cache::SeedCopy::Reflink)]
    pub seed_copy: crate::server::cache::SeedCopy,
    /// When the filesystem of a local Cargo target directory has less free
    /// space than this, GB, delete target directories that jobs built into,
    /// least recently built first, until it has 10 GB more. The mark is
    /// `--min-free-disk` when that is larger, so eviction runs whenever jobs
    /// wait for disk; a job that starts to wait makes it look at once.
    /// A directory a queued or running job builds into or whose checkout
    /// it runs in, one built in the last 2 h, and one a process uses are
    /// kept. 0 turns it off, whatever `--min-free-disk` is. `slot
    /// evict-below` replaces this mark at run time.
    #[arg(long, default_value_t = 40.0, value_name = "GB")]
    pub evict_below: f64,
    /// Only report in the journal and `slot status` which target
    /// directories eviction would delete. `dry_run` in
    /// `<state-dir>/evict.json`, read every minute, wins over this.
    #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
    pub evict_dry_run: bool,
    /// On `systemctl --user reload slot`, re-execute at once and hand the
    /// running and queued jobs, their slots and their clients to the new
    /// binary; `false`, or a new binary older than 0.11.0, uses the
    /// `--upgrade-hold` rule instead.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub handover: bool,
    /// Run every job in a sandbox (bubblewrap): the whole filesystem read-only
    /// and in its own pid namespace; writable are only the job's checkout, its
    /// target directory, a private /tmp and the download caches of cargo and Go.
    /// Other workspaces, other jobs' target directories and the user's
    /// runtime directory are hidden. Without a working `bwrap` the daemon
    /// does not start.
    #[arg(long)]
    pub sandbox: bool,
    /// The host configuration file: its `[remote.<name>]` tables are the
    /// remote nodes the daemon polls every 5 s and shows in `GET /api/nodes`
    /// and the web UI. `off` reads none.
    #[arg(long, env = "SLOT_CLUSTER_FILE", default_value_os_t = crate::hostconfig::default_path())]
    pub cluster_file: PathBuf,
    /// One more path a sandboxed job may write, for example a package
    /// manager's store; repeat for more.
    #[arg(long, value_name = "PATH", requires = "sandbox")]
    pub sandbox_write: Vec<PathBuf>,
    /// With --sandbox: mount each job's checkout at /work/src and its target
    /// directory at /work/target, so every worktree of a repository builds at
    /// the same paths. The job then runs without a rustc wrapper: the compile
    /// cache server outside the sandbox cannot read sources at those paths.
    #[arg(long, requires = "sandbox")]
    pub sandbox_fixed_paths: bool,
    /// A directory under which a remote client may mirror its tree at the
    /// tree's own absolute path (`SLOT_REMOTE_SAME_PATH=1` on the client), so
    /// absolute paths into the tree work on this server. Repeat for more;
    /// `SLOT_SAME_PATH_ROOTS` lists them separated by `:`. Give `slot shell`
    /// the same list.
    #[arg(
        long,
        value_name = "DIR",
        env = "SLOT_SAME_PATH_ROOTS",
        value_delimiter = ':'
    )]
    pub same_path_root: Vec<PathBuf>,
}

/// `slot proxy` options.
#[derive(Debug, Clone, Args)]
pub struct ProxyArgs {
    /// The daemon's socket.
    #[arg(long, env = "SLOT_SOCKET", default_value_os_t = paths::default_socket())]
    pub socket: PathBuf,
}

/// `slot shell` options. Give the same roots as `slot serve` if you changed them there.
#[derive(Debug, Clone, Args)]
pub struct ShellArgs {
    /// The daemon's socket, for `slot proxy`.
    #[arg(long, env = "SLOT_SOCKET", default_value_os_t = paths::default_socket())]
    pub socket: PathBuf,
    /// The daemon's state directory.
    #[arg(long, default_value_os_t = paths::default_state_dir())]
    pub state_dir: PathBuf,
    /// Where remote clients sync workspaces (default: `<state-dir>/ws`).
    #[arg(long)]
    pub ws_root: Option<PathBuf>,
    /// Target dirs of synced workspaces (default: `<state-dir>/cache`).
    #[arg(long)]
    pub cache_root: Option<PathBuf>,
    /// The daemon's web address, for the `state` step.
    #[arg(long, env = "SLOT_HTTP", default_value = "127.0.0.1:7411")]
    pub http: String,
    /// A directory under which a client may sync its tree to the tree's own
    /// absolute path and fetch from it, as with `slot serve`; repeat for more.
    #[arg(
        long,
        value_name = "DIR",
        env = "SLOT_SAME_PATH_ROOTS",
        value_delimiter = ':'
    )]
    pub same_path_root: Vec<PathBuf>,
}

/// `slot gate` options.
#[derive(Debug, Clone, Args)]
pub struct GateArgs {
    /// A directory the sandbox sees at the same path, on the filesystem of
    /// the daemon's state directory (the pool FIFO is hard-linked into
    /// it). Repeat for more sandboxes; `SLOT_GATE_DIRS` lists them
    /// separated by `:`.
    #[arg(
        long,
        required = true,
        value_name = "DIR",
        env = "SLOT_GATE_DIRS",
        value_delimiter = ':'
    )]
    pub dir: Vec<PathBuf>,
    /// The daemon's socket.
    #[arg(long, env = "SLOT_SOCKET", default_value_os_t = paths::default_socket())]
    pub socket: PathBuf,
    /// The daemon's state directory, which holds the pool FIFO.
    #[arg(long, default_value_os_t = paths::default_state_dir())]
    pub state_dir: PathBuf,
}

/// `slot hold` arguments.
#[derive(Debug, Clone, Args)]
pub struct HoldArgs {
    /// The job's hold FIFO in a gate directory.
    pub fifo: PathBuf,
}

/// `slot __exec` arguments.
#[derive(Debug, Clone, Args)]
pub struct ExecArgs {
    /// The open-file limit, `SOFT:HARD`; `unlimited` for either.
    #[arg(long, value_name = "SOFT:HARD", value_parser = crate::proto::FileLimit::parse)]
    pub nofile: crate::proto::FileLimit,
    /// The name the program sees as its own (`argv[0]`).
    #[arg(long)]
    pub arg0: OsString,
    /// The program, a resolved path, and its arguments.
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<OsString>,
}

/// `slot __siblings` arguments.
#[derive(Debug, Clone, Args)]
pub struct SiblingsArgs {
    /// The workspace root the client syncs to.
    pub ws_root: PathBuf,
    /// The mirror's workspace id.
    pub id: String,
    /// The repository key.
    pub key: String,
}

/// `slot __cargo-git` arguments.
#[derive(Debug, Clone, Args)]
pub struct CargoGitArgs {
    /// `<entry>:<rev>` per database entry.
    #[arg(required = true)]
    pub wanted: Vec<String>,
}

/// `slot run` arguments.
#[derive(Debug, Clone, Args)]
pub struct RunArgs {
    /// Slots the job holds, as `slot --tokens N`; capped at the slots in
    /// circulation. Ignored with `--local`.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub tokens: Option<u32>,
    /// The job's queue level, as `slot --priority LEVEL`.
    #[arg(long, value_name = "LEVEL", value_enum)]
    pub priority: Option<crate::types::Priority>,
    /// How long the job may run, as `slot --timeout DURATION`.
    #[arg(long, value_name = "DURATION", value_parser = crate::run_limit::parse_duration)]
    pub timeout: Option<u64>,
    /// Run the command as a warm-up (`slot warm`): set by a daemon that
    /// hands a warm-up to a node.
    #[arg(long, hide = true)]
    pub warm: bool,
    /// The command and its arguments.
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

/// `slot warm` arguments.
#[derive(Debug, Clone, Args)]
pub struct WarmArgs {
    /// The checkout of `main` to build; the working directory without it.
    pub dir: Option<PathBuf>,
}

/// `slot status` options.
#[derive(Debug, Clone, Args)]
pub struct StatusArgs {
    /// The daemon's web address.
    #[arg(long, env = "SLOT_HTTP", default_value = "127.0.0.1:7411")]
    pub http: String,
}

/// `slot headroom` options.
#[derive(Debug, Clone, Args)]
pub struct HeadroomArgs {
    /// New headroom, percent, 0 to 50; without it, prints the current one.
    pub percent: Option<u8>,
    /// Headroom during a call, percent, 0 to 50; 0 turns call mode off.
    #[arg(long, value_name = "PERCENT")]
    pub call: Option<u8>,
    /// What the headroom is a share of: `pool` caps the pool at a fixed
    /// quota; `machine` lowers the pool's quota by the CPU used outside the
    /// pool, so the whole machine keeps the headroom free.
    #[arg(long, value_enum)]
    pub base: Option<crate::types::HeadroomBase>,
    /// The daemon's web address.
    #[arg(long, env = "SLOT_HTTP", default_value = "127.0.0.1:7411")]
    pub http: String,
}

/// `slot priority` subcommands. A rule's pattern matches the repository name
/// `slot status` shows: the exact name, or a glob where `*` is any run of
/// characters. An exact name wins over a glob, a longer glob over a shorter
/// one. A rule wins over the level a job asks for itself, and a change
/// re-ranks the queued jobs.
#[derive(Debug, Clone, Subcommand)]
pub enum PriorityCmd {
    /// Give every job of the repositories PATTERN matches the level LEVEL.
    Set(PrioritySetArgs),
    /// Remove the rule for PATTERN.
    Unset(PriorityUnsetArgs),
    /// List the rules.
    List(PriorityListArgs),
    /// Set how long a queued `normal` or `low` job waits before it is
    /// ordered as `high`, while the daemon runs.
    PromoteAfter(PriorityPromoteAfterArgs),
}

/// `slot priority set` arguments.
#[derive(Debug, Clone, Args)]
pub struct PrioritySetArgs {
    /// Repository name or glob, such as `repo-a` or `web-*`.
    pub pattern: String,
    /// `urgent`, `high`, `normal` or `low`.
    #[arg(value_enum)]
    pub level: crate::types::Priority,
    /// The daemon's web address.
    #[arg(long, env = "SLOT_HTTP", default_value = "127.0.0.1:7411")]
    pub http: String,
}

/// `slot priority unset` arguments.
#[derive(Debug, Clone, Args)]
pub struct PriorityUnsetArgs {
    /// The pattern of the rule, as `slot priority list` shows it.
    pub pattern: String,
    /// The daemon's web address.
    #[arg(long, env = "SLOT_HTTP", default_value = "127.0.0.1:7411")]
    pub http: String,
}

/// `slot priority promote-after` arguments.
#[derive(Debug, Clone, Args)]
pub struct PriorityPromoteAfterArgs {
    /// Seconds; 0 never promotes a job; `default` goes back to the daemon's
    /// `--promote-after`. The value applies to the jobs already queued, is
    /// kept in `priority.json` in the state directory and wins over
    /// `--promote-after` across reloads and restarts, until `default`
    /// removes it.
    #[arg(value_name = "SECONDS")]
    pub seconds: crate::server::priority::PromoteAfter,
    /// The daemon's web address.
    #[arg(long, env = "SLOT_HTTP", default_value = "127.0.0.1:7411")]
    pub http: String,
}

/// `slot run-limit` arguments. Without any, prints the limit in force.
#[derive(Debug, Clone, Args)]
pub struct RunLimitArgs {
    /// How long a job may run, counted from its start: seconds, or a
    /// number with `s`, `m` or `h` (`90s`, `10m`, `1h`); `off` for no limit.
    /// The daemon ends a job that runs longer (SIGTERM to its processes,
    /// SIGKILL 5 s later) and the job exits 124. The limit applies to the
    /// jobs that start after the change, is kept in `limits.json` in the
    /// state directory and holds across reloads and restarts.
    #[arg(value_name = "DURATION|off")]
    pub limit: Option<crate::run_limit::Setting>,
    /// The most a job may name with `slot --timeout`; 1 h until it is set.
    #[arg(long, value_name = "DURATION", value_parser = crate::run_limit::parse_duration)]
    pub max: Option<u64>,
    /// The daemon's web address.
    #[arg(long, env = "SLOT_HTTP", default_value = "127.0.0.1:7411")]
    pub http: String,
}

/// `slot cpu-limit` arguments. Without any, prints the values in force.
#[derive(Debug, Clone, Args)]
pub struct CpuLimitArgs {
    /// Busy CPUs from which the start of a queued job is held, from 1
    /// (`17`, `17.5`); `off` for no limit, the default. The figure is the
    /// CPU time over the last 10 s of the pool's jobs (with the compilers a
    /// compile cache server runs for them) and of the rest of the compile
    /// cache server, with what jobs that started in the last 30 s are
    /// expected to add; work outside both is shown, not counted. Without
    /// arguments it prints both parts and the CPUs outside them. A job
    /// held for 10 minutes starts whatever the figure says. A mark above the
    /// host's CPU count never holds for CPU time (`--pressure` still does).
    /// The mark reaches queued jobs within 2
    /// seconds, is kept in `limits.json` in the state directory and holds
    /// across reloads and restarts.
    #[arg(value_name = "CPUs|off")]
    pub mark: Option<crate::cpu_limit::Setting>,
    /// Jobs that may always run, whatever the host's CPUs do, from 1; 4
    /// until one is named. `off` forgets it.
    #[arg(long, value_name = "N", value_parser = cpu_floor)]
    pub floor: Option<usize>,
    /// CPU pressure of the host (`some avg10` of `/proc/pressure/cpu`,
    /// percent, over 0 and at most 100) from which the start of a queued
    /// job is held as well, while the floor's jobs run; `off` forgets it,
    /// the default. On a saturated host the CPU time of the pool stops
    /// rising at its share while the pressure keeps rising. It needs a CPU
    /// mark, holds also with a mark above the host's CPU count, keeps the
    /// bound of 10 minutes, and is kept in `limits.json`. Without arguments
    /// `slot cpu-limit` prints it and the pressure now.
    #[arg(long, value_name = "PERCENT|off")]
    pub pressure: Option<crate::cpu_limit::PressureSetting>,
    /// The daemon's web address.
    #[arg(long, env = "SLOT_HTTP", default_value = "127.0.0.1:7411")]
    pub http: String,
}

/// The value of `slot cpu-limit --floor`.
fn cpu_floor(s: &str) -> Result<usize, String> {
    match s.parse::<usize>() {
        Ok(0) => Err("at least 1 job may always run".into()),
        Ok(n) => Ok(n),
        Err(_) => Err(format!("`{s}` is not a count of jobs")),
    }
}

impl CpuLimitArgs {
    /// The change the arguments name; `None` to print the values in force.
    pub fn request(&self) -> Result<Option<crate::cpu_limit::Request>, String> {
        use crate::cpu_limit::{PressureSetting, Request, Setting};
        if self.mark == Some(Setting::Off) && self.floor.is_some() {
            return Err("`off` takes no --floor: without a mark there is no floor".into());
        }
        if self.mark == Some(Setting::Off) && matches!(self.pressure, Some(PressureSetting::At(_)))
        {
            return Err(
                "`off` takes no --pressure: without a CPU mark there is no pressure mark".into(),
            );
        }
        if self.mark.is_none() && self.floor.is_none() && self.pressure.is_none() {
            return Ok(None);
        }
        Ok(Some(Request {
            cpu_busy_max: self.mark.map(Setting::value),
            cpu_busy_floor: self.floor,
            cpu_pressure_max: self.pressure.map(PressureSetting::value),
        }))
    }
}

/// `slot admission` arguments. Without any, prints the values in force.
#[derive(Debug, Clone, Args)]
pub struct AdmissionArgs {
    /// `slots`, the default: a job holds slots of the pool while it runs,
    /// and the pool's size bounds how many run. `shares`: a job asks for a
    /// share of the CPUs, predicted from earlier runs of the same command
    /// (`--tokens N` asks for N), holds no slot of its own and starts while
    /// that share is free in a budget that follows how much slower than
    /// their best the jobs run, never above the pool's CPU quota. The
    /// pool's slots then bound the build processes inside the jobs, as
    /// many as the budget has CPUs. Kept in `limits.json` in the state
    /// directory; holds across reloads and restarts.
    #[arg(value_name = "shares|slots")]
    pub mode: Option<crate::shares::Mode>,
    /// By share: jobs that run whatever the share says, from 1; 4 until
    /// one is named.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub floor: Option<u32>,
    /// By share: the most jobs that run at once, at least the floor; 48
    /// until one is named.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub max: Option<u32>,
    /// By share: CPU pressure of the host (`some avg10` of
    /// `/proc/pressure/cpu`, percent) over which only jobs predicted to run
    /// 5 s or less start; 60 until one is named.
    #[arg(long, value_name = "PERCENT")]
    pub pressure: Option<f64>,
    /// By share: the least CPUs the budget falls to, at least 1; half the
    /// host's CPUs until one is named, and never under the shares of the
    /// floor's running jobs.
    #[arg(long, value_name = "CPUS")]
    pub min_budget: Option<f64>,
    /// The daemon's web address.
    #[arg(long, env = "SLOT_HTTP", default_value = "127.0.0.1:7411")]
    pub http: String,
}

impl AdmissionArgs {
    /// The change the arguments name; `None` to print the values in force.
    pub fn request(&self) -> Option<crate::shares::Request> {
        let req = crate::shares::Request {
            admission: self.mode,
            floor: self.floor.map(|n| n as usize),
            max: self.max.map(|n| n as usize),
            pressure_max: self.pressure,
            min_budget: self.min_budget,
        };
        (req != crate::shares::Request::default()).then_some(req)
    }
}

/// `slot evict-below` arguments. Without any, prints the mark and the idle
/// keep in force and where they come from.
#[derive(Debug, Clone, Args)]
pub struct EvictBelowArgs {
    /// Free space, GB, below which eviction deletes idle target
    /// directories, from 0 (eviction off) to 100000; `default` forgets the
    /// mark and the idle keep, and `serve --evict-below` and 2 h apply
    /// again. It replaces `--evict-below` and is raised to
    /// `--min-free-disk` when that is larger; the disk hold itself does not
    /// change. Eviction looks at once. The mark is kept in `limits.json` in
    /// the state directory and holds across reloads and restarts.
    #[arg(value_name = "GB|default")]
    pub mark: Option<crate::evict::BelowSetting>,
    /// The idle keep: eviction keeps a target directory built more recently
    /// than this, from 5m to 168h (7 days); 2h until it is set. Kept in
    /// `limits.json` like the mark; applies at the next look.
    #[arg(long, value_name = "DURATION", value_parser = crate::evict::parse_idle)]
    pub idle: Option<u64>,
    /// The daemon's web address.
    #[arg(long, env = "SLOT_HTTP", default_value = "127.0.0.1:7411")]
    pub http: String,
}

/// `slot job-parallel` arguments. Without any, prints the value in force.
#[derive(Debug, Clone, Args)]
pub struct JobParallelArgs {
    /// Build processes one job may run at once, from 1; `off` for no
    /// limit, the default. With a limit N the daemon starts a job without
    /// the pool's jobserver and with `CARGO_BUILD_JOBS` and `MAKEFLAGS=-j`
    /// set to N, or to the slots the job holds if that is more (`slot
    /// --tokens`): `cargo` and `make` in the job then bound themselves and
    /// take no slots of the pool, so the pool limits how many jobs run, not
    /// how many compilers. It applies to the jobs that start after the
    /// change, is kept in `limits.json` in the state directory and holds
    /// across reloads and restarts.
    #[arg(value_name = "N|off")]
    pub limit: Option<crate::job_parallel::Setting>,
    /// The daemon's web address.
    #[arg(long, env = "SLOT_HTTP", default_value = "127.0.0.1:7411")]
    pub http: String,
}

/// `slot priority list` options.
#[derive(Debug, Clone, Args)]
pub struct PriorityListArgs {
    /// The daemon's web address.
    #[arg(long, env = "SLOT_HTTP", default_value = "127.0.0.1:7411")]
    pub http: String,
}

/// `slot cancel` arguments.
#[derive(Debug, Clone, Args)]
pub struct CancelArgs {
    /// Job id, as `slot status` shows it (`#12` or `12`).
    #[arg(value_parser = parse_job_id)]
    pub id: i64,
}

fn parse_job_id(s: &str) -> Result<i64, String> {
    s.trim_start_matches('#')
        .parse()
        .map_err(|_| format!("not a job id: {s}"))
}

/// `slot sessions` options.
#[derive(Debug, Clone, Args)]
pub struct SessionsArgs {
    /// The daemon's web address.
    #[arg(long, env = "SLOT_HTTP", default_value = "127.0.0.1:7411")]
    pub http: String,
    /// Seconds of history to average.
    #[arg(long, default_value_t = 60)]
    pub window: usize,
    /// Leave out sessions below this many cores outside slot, unless a command of theirs runs now;
    /// builds outside any agent session are left out below it in any case.
    #[arg(long, default_value_t = 0.5)]
    pub min_cores: f64,
    /// Print JSON instead of text.
    #[arg(long)]
    pub json: bool,
}

/// `slot usage` options.
#[derive(Debug, Clone, Args)]
pub struct UsageArgs {
    /// The daemon's web address.
    #[arg(long, env = "SLOT_HTTP", default_value = "127.0.0.1:7411")]
    pub http: String,
    /// Window ending now: a number followed by m, h or d.
    #[arg(long, default_value = "24h")]
    pub since: String,
    /// Group by project (the namespace of the repository's remote), repo or session.
    #[arg(long, default_value = "project")]
    pub by: String,
    /// Rows to print.
    #[arg(long, default_value_t = 15)]
    pub top: usize,
    /// Print JSON instead of text.
    #[arg(long)]
    pub json: bool,
}

/// `slot cache` subcommands.
#[derive(Debug, Clone, Subcommand)]
pub enum CacheCmd {
    /// The compiler calls the sccache servers refused to cache, per
    /// repository and reason, with hits and misses, and what a repository
    /// can do about each reason.
    ///
    /// Reads `cache.jsonl` and `jobs.jsonl` in the state directory. A
    /// minute's calls belong to the jobs that ran in it with compile-server
    /// CPU: to one repository exactly when all of them are its, else split
    /// by each job's compile-server CPU rate times its time in the minute
    /// (shared). Minutes no such job explains are counted apart.
    Refused(CacheRefusedArgs),
}

/// `slot cache refused` options.
#[derive(Debug, Clone, Args)]
pub struct CacheRefusedArgs {
    /// The daemon's state directory.
    #[arg(long, default_value_os_t = paths::default_state_dir())]
    pub state_dir: PathBuf,
    /// Window ending now: a number followed by m, h or d.
    #[arg(long, default_value = "24h")]
    pub since: String,
    /// Group by repo or project (the namespace of the repository's remote).
    #[arg(long, default_value = "repo")]
    pub by: String,
    /// Print JSON instead of text.
    #[arg(long)]
    pub json: bool,
}

/// `slot node` arguments: a subcommand, or the arguments of a Node.js build
/// command.
#[derive(Debug, Clone, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct NodeArgs {
    /// What to do with the registry of build nodes.
    #[command(subcommand)]
    pub cmd: Option<NodeCmd>,
    /// The arguments of `node` when they name no subcommand: `slot node
    /// build.js` runs `node build.js` as a build.
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        hide = true,
        value_name = "ARGS"
    )]
    pub script: Vec<String>,
}

/// `slot node` subcommands. Each reads or writes the host configuration
/// file (`--cluster-file`, else `SLOT_CLUSTER_FILE`, else
/// `$XDG_CONFIG_HOME/slot/cluster.toml`), the file the daemon reads; the
/// daemon takes a change in at its next poll, within 5 s.
#[derive(Debug, Clone, Subcommand)]
pub enum NodeCmd {
    /// List the registered nodes: name, state as the daemon sees it (when
    /// it answers), whether it takes queued local jobs (spill), whether it
    /// is the default node, and the repositories routed to it.
    List(NodeListArgs),
    /// Print the node a job in DIR goes to, as shell assignments of
    /// `SLOT_HOST`, `SLOT_SSH`, `SLOT_REMOTE` and `SLOT_NODE_END` (the
    /// node's deadline, Unix seconds; left out without one) and an
    /// `export` line, for `eval "$(slot node route)"`.
    ///
    /// The node is the entry whose `repositories` lists the repository of
    /// DIR (the name `slot status` shows), else `default_node`. Reads only
    /// the file: no daemon and no network. Exits 3 with one line naming
    /// the file when no entry matches or the matched name has no entry.
    Route(NodeRouteArgs),
    /// Register a node: write its `[remote.<name>]` entry. A repository
    /// listed under another node moves to this one. The rest of the file
    /// stays as it was; a file that does not parse is refused.
    Add(NodeAddArgs),
    /// Deregister a node: take its entry out. Its repositories then go to
    /// `default_node`; removing the default node clears `default_node`.
    Remove(NodeRemoveArgs),
}

/// `slot node list` arguments.
#[derive(Debug, Clone, Args)]
pub struct NodeListArgs {
    /// The host configuration file.
    #[arg(long, env = "SLOT_CLUSTER_FILE", default_value_os_t = crate::hostconfig::default_path())]
    pub cluster_file: PathBuf,
    /// The daemon's web address, for each node's state.
    #[arg(long, env = "SLOT_HTTP", default_value = "127.0.0.1:7411")]
    pub http: String,
}

/// `slot node route` arguments.
#[derive(Debug, Clone, Args)]
pub struct NodeRouteArgs {
    /// A directory of the checkout; the working directory when absent.
    pub dir: Option<PathBuf>,
    /// The host configuration file.
    #[arg(long, env = "SLOT_CLUSTER_FILE", default_value_os_t = crate::hostconfig::default_path())]
    pub cluster_file: PathBuf,
}

/// `slot node add` arguments.
#[derive(Debug, Clone, Args)]
pub struct NodeAddArgs {
    /// Node name: lower-case letters, digits and `-`.
    pub name: String,
    /// The ssh command line that reaches the node, host last
    /// (`ssh -o BatchMode=yes build-1`).
    #[arg(long)]
    pub ssh: String,
    /// Path of `slot` on the node; `slot` when absent.
    #[arg(long)]
    pub slot_path: Option<String>,
    /// End of the node's lease, RFC 3339.
    #[arg(long)]
    pub deadline: Option<String>,
    /// Price per hour, cents.
    #[arg(long)]
    pub price_per_hour_cents: Option<u32>,
    /// `false`: the daemon never hands a queued local job to the node.
    #[arg(long, value_name = "true|false")]
    pub spill: Option<bool>,
    /// A repository whose jobs go to the node; repeat for more.
    #[arg(long, value_name = "NAME")]
    pub repository: Vec<String>,
    /// Make the node `default_node`: the node of every repository no entry
    /// lists.
    #[arg(long)]
    pub default: bool,
    /// The host configuration file.
    #[arg(long, env = "SLOT_CLUSTER_FILE", default_value_os_t = crate::hostconfig::default_path())]
    pub cluster_file: PathBuf,
}

/// `slot node remove` arguments.
#[derive(Debug, Clone, Args)]
pub struct NodeRemoveArgs {
    /// Node name.
    pub name: String,
    /// The host configuration file.
    #[arg(long, env = "SLOT_CLUSTER_FILE", default_value_os_t = crate::hostconfig::default_path())]
    pub cluster_file: PathBuf,
}

/// `slot hook` subcommands.
#[derive(Debug, Clone, Subcommand)]
pub enum HookCmd {
    /// Claude Code `PreToolUse` hook for the Bash tool: reads the event on
    /// stdin and exits 2 with the `slot` command to run when the command
    /// builds or tests outside the pool. `SLOT_HOOK=warn` adds the advice as
    /// context instead of refusing; `SLOT_HOOK=off` allows everything.
    ClaudePreToolUse,
}

/// Gigabytes (10⁹ bytes) to bytes; negative or not a number is 0.
fn gb_bytes(gb: f64) -> u64 {
    if gb.is_finite() && gb > 0.0 {
        (gb * crate::admission::GB as f64) as u64
    } else {
        0
    }
}

fn default_tokens() -> usize {
    crate::procstat::host_cpus()
}

impl ServeArgs {
    /// The daemon configuration these options describe.
    pub fn config(&self) -> anyhow::Result<crate::server::Config> {
        let http: Option<SocketAddr> = if self.http == "off" || self.http.is_empty() {
            None
        } else {
            Some(self.http.parse()?)
        };
        let tokens = self.tokens.unwrap_or_else(default_tokens);
        Ok(crate::server::Config {
            socket: self.socket.clone(),
            http,
            tokens,
            go_tokens: self.go_tokens,
            test_threads: self.test_threads.unwrap_or((tokens / 4).max(2)),
            cpu_quota: self.cpu_quota,
            ws_root: self
                .ws_root
                .clone()
                .unwrap_or_else(|| self.state_dir.join("ws")),
            cache_root: self
                .cache_root
                .clone()
                .unwrap_or_else(|| self.state_dir.join("cache")),
            state_dir: self.state_dir.clone(),
            memory_max: self.memory_max.clone(),
            keep_jobs: self.keep_jobs,
            cpu_weight: (self.cpu_weight > 0).then_some(self.cpu_weight),
            call_headroom: self.call_headroom,
            call_off_after: std::time::Duration::from_secs(self.call_off_after),
            call_ignore: self.call_ignore.clone(),
            sccache_watch: self.sccache_watch,
            admission: crate::admission::Limits {
                min_free_memory: gb_bytes(self.min_free_memory),
                min_free_disk: gb_bytes(self.min_free_disk),
                min_free_disk_light: gb_bytes(
                    self.min_free_disk_light.unwrap_or(self.min_free_disk),
                ),
            },
            upgrade_hold: std::time::Duration::from_secs(self.upgrade_hold),
            promote_after: std::time::Duration::from_secs(self.promote_after),
            pass_for: std::time::Duration::from_secs(self.pass_for),
            clock: crate::server::now_ms,
            target_poll: crate::server::TARGET_POLL,
            target_settle: crate::server::TARGET_SETTLE,
            token_watch: crate::server::TOKEN_WATCH,
            resource_poll: crate::server::RESOURCE_POLL,
            cpu_busy: None,
            host_cpus: None,
            cpu_hold_max: crate::cpu_limit::HOLD_MAX,
            cpu_bound_gap: std::time::Duration::from_millis(crate::cpu_limit::STARTING_MS as u64),
            cpu_pressure: None,
            cache_budget: gb_bytes(self.cache_budget),
            seed_copy: self.seed_copy,
            evict: crate::evict::Settings {
                floor: gb_bytes(self.evict_below),
                margin: 10 * crate::admission::GB,
                min_idle: crate::evict::DEFAULT_MIN_IDLE,
                dry_run: self.evict_dry_run,
            },
            disk_free: None,
            exec_shim: Some(PathBuf::from("/proc/self/exe")),
            handover: self.handover,
            sandbox: if self.sandbox {
                Some(crate::sandbox::Sandbox::find(
                    self.sandbox_write.clone(),
                    self.sandbox_fixed_paths,
                )?)
            } else {
                None
            },
            cluster_file: (self.cluster_file.as_os_str() != "off")
                .then(|| self.cluster_file.clone()),
            same_path_roots: self.same_path_root.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("slot").chain(args.iter().copied()))
    }

    #[test]
    fn here_is_a_global_flag_off_by_default() {
        let cli = parse(&["--here", "git", "fetch"]).unwrap();
        assert!(cli.here);
        assert!(!cli.local);
        assert!(matches!(cli.command, Cmd::External(_)));
        let cli = parse(&["run", "--here", "--", "git", "push"]).unwrap();
        assert!(cli.here);
        assert!(matches!(cli.command, Cmd::Run(_)));
        assert!(!parse(&["git", "clone", "x"]).unwrap().here);
    }

    #[test]
    fn node_takes_its_subcommands_and_runs_any_other_node_command_as_a_build() {
        let Cmd::Node(a) = parse(&["node", "route", "/w/repo"]).unwrap().command else {
            panic!("not node")
        };
        assert!(
            matches!(a.cmd, Some(NodeCmd::Route(ref r)) if r.dir.as_deref() == Some(std::path::Path::new("/w/repo")))
        );
        let Cmd::Node(a) = parse(&[
            "node",
            "add",
            "build-2",
            "--ssh",
            "ssh build-2",
            "--spill",
            "false",
            "--repository",
            "repo-a",
            "--repository",
            "repo-b",
            "--default",
        ])
        .unwrap()
        .command
        else {
            panic!("not node")
        };
        let Some(NodeCmd::Add(add)) = a.cmd else {
            panic!("not add")
        };
        assert_eq!(add.name, "build-2");
        assert_eq!(add.spill, Some(false));
        assert_eq!(add.repository, ["repo-a", "repo-b"]);
        assert!(add.default);
        for (args, script) in [
            (
                &["node", "--test", "a.test.ts"][..],
                &["--test", "a.test.ts"][..],
            ),
            (&["node", "tools/ux.mjs", "gate"], &["tools/ux.mjs", "gate"]),
            (&["--tokens", "2", "node", "build.js"], &["build.js"]),
        ] {
            let Cmd::Node(a) = parse(args).unwrap().command else {
                panic!("not node: {args:?}")
            };
            assert!(a.cmd.is_none(), "{args:?}");
            assert_eq!(a.script, script, "{args:?}");
        }
    }

    #[test]
    fn eviction_deletes_by_default() {
        let Cmd::Serve(a) = parse(&["serve"]).unwrap().command else {
            panic!("not serve")
        };
        assert!(!a.evict_dry_run);
        let Cmd::Serve(a) = parse(&["serve", "--evict-dry-run", "true"])
            .unwrap()
            .command
        else {
            panic!("not serve")
        };
        assert!(a.evict_dry_run);
    }

    #[test]
    fn the_exec_shim_takes_the_jobs_argv_as_it_is() {
        let cli = parse(&[
            "__exec",
            "--nofile",
            "256:unlimited",
            "--arg0",
            "cc",
            "--",
            "/usr/bin/sh",
            "-c",
            "--nofile 1",
            "--",
        ])
        .unwrap();
        let Cmd::Exec(a) = cli.command else {
            panic!("not __exec")
        };
        assert_eq!(
            a.nofile,
            crate::proto::FileLimit {
                soft: 256,
                hard: u64::MAX
            }
        );
        assert_eq!(a.arg0, "cc");
        assert_eq!(a.command, ["/usr/bin/sh", "-c", "--nofile 1", "--"]);
        let help = <Cli as clap::CommandFactory>::command()
            .render_long_help()
            .to_string();
        assert!(!help.contains("__exec"), "hidden: {help}");
    }

    #[test]
    fn job_tokens_parse_before_a_command_and_on_run() {
        let cli = parse(&["--tokens", "4", "sh", "build.sh"]).unwrap();
        assert_eq!(cli.tokens, Some(4));
        assert!(matches!(cli.command, Cmd::External(ref a) if a == &["sh", "build.sh"]));
        let cli = parse(&["run", "--tokens", "3", "sh", "--tokens", "9"]).unwrap();
        let Cmd::Run(run) = cli.command else {
            panic!("not run")
        };
        assert_eq!(run.tokens, Some(3));
        assert_eq!(run.command, ["sh", "--tokens", "9"]);
        let cli = parse(&["sh", "build.sh", "--tokens", "2"]).unwrap();
        assert_eq!(cli.tokens, None, "after the command it is the command's");
        assert!(parse(&["--tokens", "0", "sh"]).is_err());
        assert!(parse(&["run", "--tokens", "x", "sh"]).is_err());
    }

    #[test]
    fn job_priority_parses_before_a_command_and_on_run() {
        use crate::types::Priority;
        let cli = parse(&["--priority", "high", "sh", "build.sh"]).unwrap();
        assert_eq!(cli.priority, Some(Priority::High));
        let cli = parse(&["run", "--priority", "low", "sh", "--priority", "x"]).unwrap();
        let Cmd::Run(run) = cli.command else {
            panic!("not run")
        };
        assert_eq!(run.priority, Some(Priority::Low));
        assert_eq!(run.command, ["sh", "--priority", "x"]);
        let cli = parse(&["--priority", "normal", "true"]).unwrap();
        assert_eq!(cli.priority, Some(Priority::Normal));
        let cli = parse(&["--priority", "urgent", "sh"]).unwrap();
        assert_eq!(cli.priority, Some(Priority::Urgent));
        assert!(parse(&["--priority", "asap", "sh"]).is_err());
        let cli = parse(&["priority", "set", "web-*", "low"]).unwrap();
        let Cmd::Priority(PriorityCmd::Set(a)) = cli.command else {
            panic!("not priority set")
        };
        assert_eq!((a.pattern.as_str(), a.level), ("web-*", Priority::Low));
    }

    #[test]
    fn priority_promote_after_takes_seconds_or_default() {
        use crate::server::priority::PromoteAfter;
        let seconds = |args: &[&str]| {
            let mut argv = vec!["priority", "promote-after"];
            argv.extend(args);
            parse(&argv).map(|cli| {
                let Cmd::Priority(PriorityCmd::PromoteAfter(a)) = cli.command else {
                    panic!("not priority promote-after")
                };
                a.seconds
            })
        };
        assert_eq!(seconds(&["300"]).unwrap(), PromoteAfter::Seconds(300));
        assert_eq!(seconds(&["0"]).unwrap(), PromoteAfter::Seconds(0));
        assert_eq!(seconds(&["default"]).unwrap(), PromoteAfter::Default);
        assert!(seconds(&[]).is_err(), "the value is required");
        assert!(seconds(&["soon"]).is_err());
        assert!(seconds(&["-1"]).is_err());
        assert!(seconds(&["5m"]).is_err());
    }

    #[test]
    fn a_jobs_timeout_parses_before_a_command_and_on_run() {
        let cli = parse(&["--timeout", "30m", "sh", "build.sh"]).unwrap();
        assert_eq!(cli.timeout, Some(1800));
        assert!(matches!(cli.command, Cmd::External(ref a) if a == &["sh", "build.sh"]));
        let cli = parse(&["--timeout", "90", "true"]).unwrap();
        assert_eq!(cli.timeout, Some(90));
        let cli = parse(&["run", "--timeout", "1h", "sh", "--timeout", "9"]).unwrap();
        let Cmd::Run(run) = cli.command else {
            panic!("not run")
        };
        assert_eq!(run.timeout, Some(3600));
        assert_eq!(run.command, ["sh", "--timeout", "9"]);
        let cli = parse(&["timeout", "1800", "cargo", "test"]).unwrap();
        assert_eq!(cli.timeout, None, "a `timeout` command is the job");
        assert!(
            matches!(cli.command, Cmd::External(ref a) if a == &["timeout", "1800", "cargo", "test"])
        );
        assert!(parse(&["--timeout", "0", "sh"]).is_err());
        assert!(parse(&["--timeout", "soon", "sh"]).is_err());
        assert!(parse(&["run", "--timeout", "off", "sh"]).is_err());
    }

    #[test]
    fn run_limit_takes_a_duration_or_off_and_a_maximum() {
        use crate::run_limit::Setting;
        let args = |args: &[&str]| {
            let mut argv = vec!["run-limit"];
            argv.extend(args);
            parse(&argv).map(|cli| {
                let Cmd::RunLimit(a) = cli.command else {
                    panic!("not run-limit")
                };
                (a.limit, a.max)
            })
        };
        assert_eq!(args(&[]).unwrap(), (None, None), "prints the limit");
        assert_eq!(args(&["10m"]).unwrap(), (Some(Setting::Seconds(600)), None));
        assert_eq!(args(&["600"]).unwrap(), (Some(Setting::Seconds(600)), None));
        assert_eq!(args(&["off"]).unwrap(), (Some(Setting::Off), None));
        assert_eq!(
            args(&["10m", "--max", "1h"]).unwrap(),
            (Some(Setting::Seconds(600)), Some(3600))
        );
        assert_eq!(args(&["--max", "2h"]).unwrap(), (None, Some(7200)));
        assert!(args(&["0"]).is_err());
        assert!(args(&["soon"]).is_err());
        assert!(args(&["10m", "--max", "off"]).is_err());
        assert!(args(&["10m", "--max", "0"]).is_err());
    }

    #[test]
    fn evict_below_takes_gb_or_default() {
        use crate::evict::BelowSetting;
        let mark = |args: &[&str]| {
            let mut argv = vec!["evict-below"];
            argv.extend(args);
            parse(&argv).map(|cli| {
                let Cmd::EvictBelow(a) = cli.command else {
                    panic!("not evict-below")
                };
                a.mark
            })
        };
        assert_eq!(mark(&[]).unwrap(), None, "prints the mark in force");
        assert_eq!(mark(&["100"]).unwrap(), Some(BelowSetting::At(100.0)));
        assert_eq!(mark(&["0"]).unwrap(), Some(BelowSetting::At(0.0)));
        assert_eq!(mark(&["default"]).unwrap(), Some(BelowSetting::Default));
        for bad in [&["-1"][..], &["200000"], &["lots"], &["1", "2"]] {
            assert!(mark(bad).is_err(), "{bad:?}");
        }
        let cli = parse(&["evict-below", "80", "--http", "127.0.0.1:9"]).unwrap();
        assert!(matches!(cli.command, Cmd::EvictBelow(ref a) if a.http == "127.0.0.1:9"));
    }

    #[test]
    fn evict_below_takes_an_idle_keep() {
        use crate::evict::BelowSetting;
        let args = |args: &[&str]| {
            let mut argv = vec!["evict-below"];
            argv.extend(args);
            parse(&argv).map(|cli| {
                let Cmd::EvictBelow(a) = cli.command else {
                    panic!("not evict-below")
                };
                (a.mark, a.idle)
            })
        };
        assert_eq!(args(&[]).unwrap(), (None, None));
        assert_eq!(
            args(&["100", "--idle", "30m"]).unwrap(),
            (Some(BelowSetting::At(100.0)), Some(1800))
        );
        assert_eq!(args(&["--idle", "1h"]).unwrap(), (None, Some(3600)));
        assert_eq!(args(&["--idle", "168h"]).unwrap(), (None, Some(604_800)));
        assert_eq!(args(&["--idle", "300"]).unwrap(), (None, Some(300)));
        for bad in ["4m", "169h", "0", "soon", "default"] {
            assert!(args(&["100", "--idle", bad]).is_err(), "{bad}");
        }
    }

    #[test]
    fn job_parallel_takes_a_count_or_off() {
        use crate::job_parallel::Setting;
        let limit = |args: &[&str]| {
            let mut argv = vec!["job-parallel"];
            argv.extend(args);
            parse(&argv).map(|cli| {
                let Cmd::JobParallel(a) = cli.command else {
                    panic!("not job-parallel")
                };
                a.limit
            })
        };
        assert_eq!(limit(&[]).unwrap(), None, "prints the value in force");
        assert_eq!(limit(&["4"]).unwrap(), Some(Setting::At(4)));
        assert_eq!(limit(&["1"]).unwrap(), Some(Setting::At(1)));
        assert_eq!(limit(&["off"]).unwrap(), Some(Setting::Off));
        assert!(limit(&["0"]).is_err(), "0 is refused");
        assert!(limit(&["four"]).is_err());
        assert!(limit(&["4", "5"]).is_err());
        let cli = parse(&["job-parallel", "2", "--http", "127.0.0.1:9"]).unwrap();
        assert!(matches!(cli.command, Cmd::JobParallel(ref a) if a.http == "127.0.0.1:9"));
    }

    #[test]
    fn serve_keeps_its_own_tokens() {
        let cli = parse(&["serve", "--tokens", "8"]).unwrap();
        let Cmd::Serve(s) = cli.command else {
            panic!("not serve")
        };
        assert_eq!(s.tokens, Some(8));
    }

    #[test]
    fn cpu_limit_takes_a_mark_or_off_and_a_floor() {
        use crate::cpu_limit::{Request, Setting};
        let args = |args: &[&str]| {
            let mut argv = vec!["cpu-limit"];
            argv.extend(args);
            parse(&argv).map(|cli| {
                let Cmd::CpuLimit(a) = cli.command else {
                    panic!("not cpu-limit")
                };
                a
            })
        };
        let change = |mark: Option<Option<f64>>, floor: Option<usize>| {
            Ok(Some(Request {
                cpu_busy_max: mark,
                cpu_busy_floor: floor,
                ..Request::default()
            }))
        };
        let pressure = |p: Option<f64>| {
            Ok(Some(Request {
                cpu_pressure_max: Some(p),
                ..Request::default()
            }))
        };
        assert_eq!(
            args(&["--pressure", "80"]).unwrap().request(),
            pressure(Some(80.0)),
            "the pressure mark alone"
        );
        assert_eq!(
            args(&["--pressure", "off"]).unwrap().request(),
            pressure(None)
        );
        assert_eq!(
            args(&["17", "--pressure", "62.5"]).unwrap().request(),
            Ok(Some(Request {
                cpu_busy_max: Some(Some(17.0)),
                cpu_pressure_max: Some(Some(62.5)),
                ..Request::default()
            }))
        );
        let e = args(&["off", "--pressure", "80"])
            .unwrap()
            .request()
            .unwrap_err();
        assert!(e.contains("takes no --pressure"), "{e}");
        for bad in [
            &["--pressure", "0"][..],
            &["--pressure", "101"],
            &["--pressure"],
        ] {
            assert!(args(bad).is_err(), "{bad:?}");
        }
        let a = args(&[]).unwrap();
        assert_eq!((a.mark, a.floor), (None, None));
        assert_eq!(a.request(), Ok(None), "prints the values in force");
        let a = args(&["17"]).unwrap();
        assert_eq!(a.mark, Some(Setting::At(17.0)));
        assert_eq!(a.request(), change(Some(Some(17.0)), None));
        let a = args(&["17.5", "--floor", "6"]).unwrap();
        assert_eq!(a.request(), change(Some(Some(17.5)), Some(6)));
        let a = args(&["--floor", "6"]).unwrap();
        assert_eq!(a.request(), change(None, Some(6)), "the floor alone");
        let a = args(&["off"]).unwrap();
        assert_eq!(a.mark, Some(Setting::Off));
        assert_eq!(a.request(), change(Some(None), None));
        let e = args(&["off", "--floor", "3"])
            .unwrap()
            .request()
            .unwrap_err();
        assert!(e.contains("takes no --floor"), "{e}");
        for bad in [
            &["0"][..],
            &["0.5"],
            &["nan"],
            &["inf"],
            &["many"],
            &["17", "18"],
            &["17", "--floor", "0"],
            &["17", "--floor", "some"],
            &["17", "--floor"],
        ] {
            assert!(args(bad).is_err(), "{bad:?}");
        }
        let cli = parse(&["cpu-limit", "8", "--http", "127.0.0.1:9"]).unwrap();
        assert!(matches!(cli.command, Cmd::CpuLimit(ref a) if a.http == "127.0.0.1:9"));
    }

    #[test]
    fn admission_takes_shares_or_slots_the_bounds_and_the_pressure_mark() {
        use crate::shares::{Mode, Request};
        let req = |argv: &[&str]| {
            let mut all = vec!["admission"];
            all.extend_from_slice(argv);
            parse(&all).map(|cli| {
                let Cmd::Admission(a) = cli.command else {
                    panic!("not admission")
                };
                a.request()
            })
        };
        assert_eq!(req(&[]).unwrap(), None, "prints the values in force");
        assert_eq!(
            req(&["shares"]).unwrap(),
            Some(Request {
                admission: Some(Mode::Shares),
                ..Request::default()
            })
        );
        assert_eq!(
            req(&["slots"]).unwrap().and_then(|r| r.admission),
            Some(Mode::Slots)
        );
        assert_eq!(
            req(&["--floor", "2", "--max", "30", "--pressure", "45"]).unwrap(),
            Some(Request {
                admission: None,
                floor: Some(2),
                max: Some(30),
                pressure_max: Some(45.0),
                min_budget: None,
            })
        );
        for bad in [
            &["cores"][..],
            &["Shares"],
            &["--floor", "0"],
            &["--max", "0"],
            &["--max", "-1"],
            &["--pressure"],
        ] {
            assert!(req(bad).is_err(), "{bad:?}");
        }
    }
}
