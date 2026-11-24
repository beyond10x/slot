//! Types the daemon sends to the web UI. TypeScript copies in
//! `ui/src/generated/` are generated from these by `slot-docs ts-types` and
//! drift-checked by `task check`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Lifecycle state of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    /// Waiting for slots.
    Queued,
    /// Running.
    Running,
    /// Exited 0.
    Ok,
    /// Exited non-zero or could not start.
    Failed,
    /// The client went away.
    Cancelled,
}

impl JobState {
    /// Whether the job has ended.
    pub fn ended(self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }
}

/// Git checkout containing the submitting client's working directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Checkout {
    /// The repository's main working tree.
    Repository,
    /// A linked working tree of the repository.
    Worktree {
        /// Name of the linked working directory.
        name: String,
    },
    /// The working directory is outside a Git checkout.
    Directory,
}

/// One job, as logged and as shown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
// Each flag is a fact of its own that the log and the views read alone.
#[allow(clippy::struct_excessive_bools)]
pub struct Job {
    /// Job id, increasing.
    #[ts(type = "number")]
    pub id: i64,
    /// Command and arguments.
    pub argv: Vec<String>,
    /// Working directory on the daemon's host.
    pub cwd: String,
    /// Repository name.
    pub repo: String,
    /// How the client identified its checkout. Absent for unverified jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub checkout: Option<Checkout>,
    /// Project: the namespace of the repository's remote, or `.slot.toml`'s
    /// `project`; empty when unknown (`crate::project`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project: String,
    /// Agent session id.
    #[serde(default)]
    pub session: String,
    /// `host:pid` of the client's parent.
    #[serde(default)]
    pub client: String,
    /// Toolchain name.
    pub toolchain: String,
    /// Whether the source was synced from another host.
    pub remote: bool,
    /// Slots the job holds.
    pub tokens: u32,
    /// Lifecycle state.
    pub state: JobState,
    /// Enqueue time, Unix milliseconds.
    #[ts(type = "number")]
    pub enqueued_ms: i64,
    /// Start time, Unix milliseconds; 0 if it never started.
    #[ts(type = "number")]
    pub started_ms: i64,
    /// End time, Unix milliseconds; 0 while running.
    #[ts(type = "number")]
    pub ended_ms: i64,
    /// Exit code; -1 when cancelled.
    pub exit: i32,
    /// CPU seconds of the job's tree plus compile-server work.
    pub cpu_s: f64,
    /// Sampled CPU seconds spent in compile servers (part of `cpu_s`).
    pub compile_server_cpu_s: f64,
    /// Highest sampled resident memory, bytes.
    #[ts(type = "number")]
    pub peak_rss: u64,
    /// Why a queued job waits besides slots (disk space, memory, a build in
    /// its target directory); empty otherwise.
    #[serde(default)]
    pub wait_reason: String,
    /// Cargo target directory a local Rust job builds into; empty otherwise.
    #[serde(default)]
    pub target_dir: String,
    /// The level a queued job is ordered by: the rule for its repository,
    /// else its own `--priority`, raised to `high` once it waited longer
    /// than the promotion delay (`--promote-after`, or the one set with
    /// `slot priority promote-after`). `normal` from a daemon without levels.
    #[serde(default)]
    pub priority: Priority,
    /// Whether `priority` is `high` only because the job waited that long.
    #[serde(default)]
    pub promoted: bool,
    /// Rust jobs: the variables of the job's environment that decide what
    /// the compile cache can do (`crate::sccache::CACHE_VARS` and every
    /// `CARGO_PROFILE_*`), each as the job got it; a variable that is not
    /// set is absent. Absent for other toolchains and from a daemon that
    /// does not record it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cache_env: Option<BTreeMap<String, String>>,
    /// What is unusual about the job's compile cache setup, one short text
    /// each (`crate::sccache::cache_warnings`); the job runs unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Seconds the job may run from its start before the daemon ends it:
    /// its own `--timeout`, else the daemon's limit (`crate::run_limit`).
    /// Absent while the job is queued, for a job without a limit and from a
    /// daemon without the run-time limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub limit_s: Option<u64>,
    /// Whether the daemon ended the job at `limit_s`; its exit code is then
    /// 124.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub timed_out: bool,
    /// The run time the daemon predicted when the job queued, ms: the 90th
    /// percentile of the last runs of the same command in the same
    /// repository (`admission::History::expected_runtime`). Absent without
    /// enough history and from a daemon that does not predict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub predicted_ms: Option<u64>,
    /// Slots a queued job has collected towards its `tokens` while it
    /// waits; 0 once it runs and from a daemon that does not say.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub collected: u32,
    /// Whether the job was moved ahead of a job that priority, then arrival
    /// serves first: as a job predicted to run a few seconds, or as one
    /// that fits in slots a larger request collected and cannot use yet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ahead: bool,
    /// Where a queued job stands inside its level in the pool's queue: 0
    /// may no longer be passed (it waited `--pass-for`), 1 predicted short,
    /// 2 the others. The pool serves by level, then this, then arrival; a
    /// view lists the queue in that order. Absent once the job runs, while
    /// it has not been in the pool's queue, and from a daemon that does
    /// not say: a view then takes `ahead` as 1 and anything else as 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub queue_class: Option<u8>,
    /// The share of the CPUs the job asked for while admission is by share
    /// (`slot admission shares`): predicted from earlier runs of the same
    /// command, or the slots its client named. Absent with admission by
    /// slots and from a daemon without shares.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub share: Option<f64>,
    /// CPUs the job used over the last 5 s while it runs, and over its
    /// whole run once it ended; with `share` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub share_used: Option<f64>,
    /// CPU-seconds of its share the job lent to the queue while it used
    /// almost no CPU; with `share` only.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub lent_s: f64,
    /// How the job was admitted when it started: by share it held no
    /// slot, and it is charged and counted by share until it ends, also
    /// after a switch back to slots. Absent by slots and from a daemon
    /// without shares.
    #[serde(default, skip_serializing_if = "crate::shares::Mode::is_slots")]
    pub admission: crate::shares::Mode,
    /// Id of the node the job runs or ran on ([`Node::node_id`]): `local`
    /// for every job of a daemon without remote nodes. Empty in a job-log
    /// line written before nodes; the API shows such a job as `local`.
    #[serde(default)]
    pub node: String,
}

/// Id of the daemon's own machine as a node.
pub const LOCAL_NODE: &str = "local";

/// Where a node runs (`slot.cluster.NodeKind`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum NodeKind {
    /// Part of the endpoint's machine, reached without ssh.
    #[default]
    Local,
    /// A machine with its own daemon, reached over ssh.
    Remote,
}

/// Lifecycle state of a node (`slot.cluster.Node.State`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum NodeState {
    /// Registered; not yet seen answering with the expected version and
    /// clock.
    Joining,
    /// Takes new placements and new jobs.
    #[default]
    Ready,
    /// Takes no new placement and no new job; running jobs finish.
    Draining,
    /// Left the cluster.
    Gone,
}

/// Why a node left (`slot.cluster.GoneReason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum GoneReason {
    /// Drained and idle.
    Drained,
    /// Three missed polls or three failed syncs in a row.
    Lost,
    /// Past its deadline.
    Deadline,
    /// Refused at join for another version or a clock more than 1 s off.
    RefusedAtJoin,
    /// A spot reclaim.
    Reclaimed,
}

/// One row of `GET /api/nodes` (`slot.cluster.NodeList`): a node's specs,
/// lifetime and load. Names and units are the specification's; the answer
/// is a bare array of rows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct Node {
    /// Node name, unique in its cluster; `local` for the daemon's machine.
    pub node_id: String,
    /// The cluster, one per endpoint daemon: the daemon's host name.
    pub cluster_id: String,
    /// Where the node runs.
    pub kind: NodeKind,
    /// Lifecycle state.
    pub state: NodeState,
    /// CPUs the node gives the cluster.
    pub cpus: u32,
    /// Capacity in slots: the slots in circulation (the pool less the
    /// headroom).
    pub slots: u32,
    /// Slots held by running jobs.
    pub slots_in_use: u32,
    /// Memory of the machine, GiB.
    pub memory_gib: u32,
    /// Disk for mirrors and target directories, GiB.
    pub disk_gib: u32,
    /// The most worktrees placed on a remote node at once; absent on the
    /// local node.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_worktrees: Option<u32>,
    /// Running jobs.
    pub running_jobs: u32,
    /// Queued jobs.
    pub queued_jobs: u32,
    /// The slot version the node runs.
    pub slot_version: String,
    /// CPUs the pool's jobs and the compile cache used over the last 10 s,
    /// in thousandths of a CPU.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cpu_busy_milli: Option<u32>,
    /// Memory in use on the machine, MiB.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub memory_used_mib: Option<u32>,
    /// Free disk where the node builds, GiB.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub disk_free_gib: Option<u32>,
    /// Where a remote machine runs (a provider region or a site).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub location: Option<String>,
    /// The machine type of a remote machine.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub machine_type: Option<String>,
    /// Price of a remote machine per hour, cents.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub price_per_hour_cents: Option<u32>,
    /// When the node joined, RFC 3339; the daemon's start for the local
    /// node.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub joined_at: Option<String>,
    /// The end of a remote node's lease, RFC 3339; absent on the local
    /// node.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub deadline: Option<String>,
    /// Why the node left; with `Gone` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub gone_reason: Option<GoneReason>,
    /// When the node left, RFC 3339; with `Gone` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub left_at: Option<String>,
}

/// Unix milliseconds as an RFC 3339 time in UTC, as the API's timestamps
/// are written.
pub fn rfc3339(ms: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .ok()
        .and_then(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_default()
}

// serde hands `skip_serializing_if` a reference.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(n: &u32) -> bool {
    *n == 0
}

// serde hands `skip_serializing_if` a reference.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_f64(n: &f64) -> bool {
    *n == 0.0
}

/// Live numbers of one running job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct LiveJob {
    /// Job id.
    #[ts(type = "number")]
    pub id: i64,
    /// Cores in use over the last second.
    pub cores: f64,
    /// Resident memory, bytes.
    #[ts(type = "number")]
    pub rss: u64,
    /// Processes in the job.
    pub procs: u32,
    /// Work running in the job now, busiest first.
    pub units: Vec<Unit>,
    /// Distinct units (crates, tests, build scripts) seen since the job started.
    pub units_seen: u32,
}

/// One unit of work running inside a job: a crate being compiled, a build
/// script, a test binary, a native compiler or a linker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct Unit {
    /// `compile`, `build-script`, `test`, `native` or `link`.
    pub kind: String,
    /// Crate, package or test binary name.
    pub name: String,
    /// Cores used over the last second.
    pub cores: f64,
    /// Seconds it has been running (sampled).
    pub secs: u32,
}

/// What the headroom is a share of.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS, clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum HeadroomBase {
    /// The pool: a fixed `CPUQuota` of `(100 − h) × cores` percent on the
    /// daemon's unit.
    #[default]
    Pool,
    /// The machine: the pool's `CPUQuota` follows the CPU used outside the
    /// pool, so the whole host keeps the headroom free.
    Machine,
}

impl HeadroomBase {
    /// `pool` or `machine`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pool => "pool",
            Self::Machine => "machine",
        }
    }
}

/// Which queued job gets free slots first: every `urgent` job before every
/// `high` one, `high` before `normal`, `normal` before `low`; first in,
/// first out inside one level.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    TS,
    clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    /// Repair work on the pool itself: starts before every other job, and
    /// with no slot free on one the headroom holds back.
    Urgent,
    /// Starts before every `normal` and `low` job.
    High,
    /// The default.
    #[default]
    Normal,
    /// Starts after every `high` and `normal` job.
    Low,
}

impl Priority {
    /// `urgent`, `high`, `normal` or `low`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Urgent => "urgent",
            Self::High => "high",
            Self::Normal => "normal",
            Self::Low => "low",
        }
    }
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// CPU of one top-level cgroup outside the pool: a desktop application, a
/// login session, a docker container or `system.slice`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct OutsideOwner {
    /// Cgroup name, for example `session-3.scope` or `docker 3f2a1b2c3d4e`.
    pub name: String,
    /// Smoothed CPUs.
    pub cores: f64,
}

/// Snapshot served at `GET /api/state`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
pub struct State {
    /// Host name.
    pub host: String,
    /// CPUs on the host.
    pub cores: u32,
    /// Pool size.
    pub tokens: u32,
    /// Tokens that circulate: the pool size less the headroom; 0 from a
    /// daemon older than 0.3.0, which has no limit.
    #[serde(default)]
    pub limit: u32,
    /// Share of the CPU kept free for interactive work, percent.
    #[serde(default)]
    pub headroom_percent: u32,
    /// Applications in a call while call mode holds the headroom up; empty
    /// otherwise.
    #[serde(default)]
    pub call_apps: Vec<String>,
    /// CPU quota on the daemon unit, percent of one core; null when not capped.
    #[serde(default)]
    pub cpu_quota_percent: Option<u32>,
    /// Tokens free now.
    pub free: u32,
    /// Queued and running jobs.
    pub active: Vec<Job>,
    /// Recently finished jobs, oldest first.
    pub done: Vec<Job>,
    /// Server time, Unix milliseconds.
    #[ts(type = "number")]
    pub now_ms: i64,
    /// Registered toolchain names.
    pub toolchains: Vec<String>,
    /// The sccache watchdog.
    #[serde(default)]
    pub sccache: SccacheWatch,
    /// Eviction of local target directories.
    #[serde(default)]
    pub evict: crate::evict::Report,
    /// A pending upgrade, as one line; empty when none.
    #[serde(default)]
    pub upgrade: String,
    /// What the headroom is a share of; `pool` from a daemon older than 0.16.
    #[serde(default)]
    pub headroom_base: HeadroomBase,
    /// Machine base: CPUs busy on the host over the last second; 0 otherwise.
    #[serde(default)]
    pub busy_cores: f64,
    /// Machine base: CPUs the host may use with the headroom kept free; 0
    /// when the machine base is not in force (pool base or no headroom).
    #[serde(default)]
    pub target_cores: f64,
    /// Machine base: smoothed CPUs used outside the pool.
    #[serde(default)]
    pub outside_cores: f64,
    /// Machine base: the busiest cgroups outside the pool, at most 5.
    #[serde(default)]
    pub outside: Vec<OutsideOwner>,
    /// Machine base: the pool's quota is held at its floor of 2 CPUs.
    #[serde(default)]
    pub quota_floor: bool,
    /// Machine base: the pool is at its floor and the host is still busier
    /// than the target.
    #[serde(default)]
    pub over_target: bool,
    /// Seconds a job may run before the daemon ends it (`slot run-limit`);
    /// absent with no limit and from a daemon without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub run_limit_s: Option<u64>,
    /// The most a job may name with `slot --timeout`, seconds; absent from a
    /// daemon without the run-time limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub run_limit_max_s: Option<u64>,
    /// Build processes one job may run at once (`slot job-parallel`): jobs
    /// then get a bound of their own in place of the pool's jobserver, and
    /// their compilers take no slots. Absent with no limit and from a
    /// daemon without the setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub job_parallel: Option<usize>,
    /// Busy CPUs of the host from which the start of a queued job is held
    /// (`slot cpu-limit`). Absent with no limit and from a daemon without
    /// the setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub cpu_busy_max: Option<f64>,
    /// Jobs that may always run under that limit, whatever the host's CPUs
    /// do. Absent with no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub cpu_busy_floor: Option<usize>,
    /// CPU pressure of the host (`some avg10`, percent) from which the
    /// start of a queued job is held as well (`slot cpu-limit --pressure`).
    /// Absent without one and from a daemon without the setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub cpu_pressure_max: Option<f64>,
    /// Tokens that neither a job nor a live process with the pool open
    /// accounts for, as the daemon counted them last: lost, and written back
    /// once two counts in a row agree. 0 from a daemon that predates it.
    #[serde(default)]
    pub tokens_unaccounted: u32,
    /// Since when tokens are unaccounted for, Unix milliseconds; 0 while
    /// there are none.
    #[serde(default)]
    #[ts(type = "number")]
    pub tokens_unaccounted_since_ms: i64,
    /// Admission by CPU share (`slot admission shares`): the budget, the
    /// free share and what idle jobs lend. Absent with admission by slots
    /// and from a daemon without shares.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub shares: Option<crate::shares::View>,
}

/// One message on `GET /api/events`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Event {
    /// A job changed state.
    Job {
        /// The job.
        job: Job,
    },
    /// Once a second.
    Tick {
        /// Server time, Unix milliseconds.
        #[ts(type = "number")]
        t: i64,
        /// Pool size.
        tokens: u32,
        /// Tokens that circulate: the pool size less the headroom; 0 from a
        /// daemon older than 0.3.0.
        #[serde(default)]
        limit: u32,
        /// Share of the CPU kept free for interactive work, percent.
        #[serde(default)]
        headroom_percent: u32,
        /// Applications in a call while call mode holds the headroom up; empty
        /// otherwise.
        #[serde(default)]
        call_apps: Vec<String>,
        /// Tokens free.
        free: u32,
        /// Jobs waiting for tokens.
        waiting: u32,
        /// Host CPU busy fraction, 0 to 1.
        sys_cpu: f64,
        /// Cores used by all running jobs.
        job_cores: f64,
        /// Running jobs.
        jobs: Vec<LiveJob>,
        /// Every build process on the host, run by slot or not.
        census: HostCensus,
        /// A pending upgrade, as one line; empty when none, and from a daemon
        /// that does not send it.
        #[serde(default)]
        upgrade: String,
        /// CPU quota on the daemon unit, percent of one core; null when not
        /// capped, and from a daemon older than 0.16.
        #[serde(default)]
        cpu_quota_percent: Option<u32>,
        /// What the headroom is a share of.
        #[serde(default)]
        headroom_base: HeadroomBase,
        /// Machine base: CPUs busy on the host over the last second.
        #[serde(default)]
        busy_cores: f64,
        /// Machine base: CPUs the host may use; 0 when not in force.
        #[serde(default)]
        target_cores: f64,
        /// Machine base: smoothed CPUs used outside the pool.
        #[serde(default)]
        outside_cores: f64,
        /// Machine base: the busiest cgroups outside the pool.
        #[serde(default)]
        outside: Vec<OutsideOwner>,
        /// Machine base: the pool's quota is held at its floor.
        #[serde(default)]
        quota_floor: bool,
        /// Machine base: at the floor and still over the target.
        #[serde(default)]
        over_target: bool,
        /// Admission by CPU share: the budget, the free share and what idle
        /// jobs lend. Absent with admission by slots and from a daemon
        /// without shares.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        shares: Option<crate::shares::View>,
    },
}

/// Build processes of one tool family.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct CensusClass {
    /// Tool family, for example `rust-compile`, `go-test`, `node-test`.
    pub class: String,
    /// Whether slot runs these processes.
    pub managed: bool,
    /// Cores over the last second.
    pub cores: f64,
    /// Resident memory, bytes.
    #[ts(type = "number")]
    pub rss: u64,
    /// Processes.
    pub procs: u32,
}

/// Build processes of one agent session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct CensusSession {
    /// Session id from the process environment; empty when none.
    pub session: String,
    /// Session title when it can be resolved locally; empty otherwise.
    pub title: String,
    /// Cores over the last second.
    pub cores: f64,
    /// Cores of processes slot does not run.
    pub unmanaged_cores: f64,
    /// Resident memory, bytes.
    #[ts(type = "number")]
    pub rss: u64,
    /// Processes.
    pub procs: u32,
}

/// The sccache watchdog: servers seen and tokens written back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct SccacheWatch {
    /// Whether the watchdog runs (`slot serve --sccache-watch`).
    pub enabled: bool,
    /// sccache servers of this user seen in the last pass.
    pub servers: u32,
    /// Servers with no free token and fewer compiles than their pool now.
    pub short: u32,
    /// Refills since the daemon started.
    pub refills: u32,
    /// Tokens written back since the daemon started.
    pub tokens: u32,
    /// Time of the last refill, Unix milliseconds; 0 if none.
    #[ts(type = "number")]
    pub last_refill_ms: i64,
}

/// Host-wide view of build work.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct HostCensus {
    /// Cores used by every process on the host.
    pub host_cores: f64,
    /// Cores used by build processes.
    pub build_cores: f64,
    /// Cores used by build processes slot does not run.
    pub unmanaged_cores: f64,
    /// CPU pressure, `some avg10`, percent.
    pub psi_cpu: f64,
    /// Memory pressure, `some avg10`, percent.
    pub psi_memory: f64,
    /// IO pressure, `some avg10`, percent.
    pub psi_io: f64,
    /// One-minute load average.
    pub load1: f64,
    /// By tool family and managed flag, busiest first.
    pub classes: Vec<CensusClass>,
    /// By session, busiest first.
    pub sessions: Vec<CensusSession>,
    /// Pool size the adaptive controller recommends (observe mode: not enforced).
    pub recommended_tokens: u32,
    /// Smoothed cores of builds slot does not run, as the controller sees them.
    pub unmanaged_smoothed: f64,
}

/// One chunk on `GET /api/jobs/{id}/output`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct Chunk {
    /// `out` or `err`.
    pub t: String,
    /// Text.
    pub b: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `/api/state` and a tick as a 0.2.0 daemon sends them: no limit, no
    /// headroom. A newer client must still read them.
    #[test]
    fn reads_state_and_tick_from_an_older_daemon() {
        let state = r#"{"host":"h","cores":4,"tokens":4,"free":3,"active":[],"done":[],
            "now_ms":1,"toolchains":["rust"]}"#;
        let s: State = serde_json::from_str(state).unwrap();
        assert_eq!((s.tokens, s.limit, s.headroom_percent), (4, 0, 0));
        assert_eq!(s.cpu_quota_percent, None);
        let tick = r#"{"type":"tick","t":1,"tokens":4,"free":3,"waiting":0,"sys_cpu":0.1,
            "job_cores":0.0,"jobs":[],"census":{"host_cores":1.0}}"#;
        let Event::Tick {
            limit,
            census,
            upgrade,
            ..
        } = serde_json::from_str(tick).unwrap()
        else {
            panic!("not a tick");
        };
        assert_eq!(limit, 0);
        assert_eq!(upgrade, "");
        assert_eq!(census.host_cores, 1.0);
        assert_eq!(census.classes.len(), 0);
    }

    /// A job-log line of 0.19.1 has neither `cache_env` nor `warnings`; a
    /// job without them is written without them, so an older reader sees
    /// the line it always saw.
    #[test]
    fn a_job_logged_before_the_cache_fields_reads_and_is_written_without_them() {
        let old = r#"{"id":3,"argv":["sh","-c","exit 3"],"cwd":"/w","repo":"repo","session":"",
            "client":"h:1","toolchain":"other","remote":false,"tokens":1,"state":"failed",
            "enqueued_ms":1,"started_ms":2,"ended_ms":3,"exit":3,"cpu_s":0.0,
            "compile_server_cpu_s":0.0,"peak_rss":0}"#;
        let mut j: Job = serde_json::from_str(old).unwrap();
        assert_eq!(j.cache_env, None);
        assert_eq!(j.warnings, Vec::<String>::new());
        let line = serde_json::to_string(&j).unwrap();
        assert!(
            !line.contains("cache_env") && !line.contains("warnings"),
            "{line}"
        );
        j.cache_env = Some(BTreeMap::from([(
            "RUSTC_WRAPPER".to_string(),
            String::new(),
        )]));
        j.warnings = vec!["compile cache off: RUSTC_WRAPPER is empty".into()];
        let line = serde_json::to_string(&j).unwrap();
        assert!(
            line.contains(r#""cache_env":{"RUSTC_WRAPPER":""}"#),
            "{line}"
        );
        assert!(
            line.contains(r#""warnings":["compile cache off: RUSTC_WRAPPER is empty"]"#),
            "{line}"
        );
        assert_eq!(serde_json::from_str::<Job>(&line).unwrap(), j);
    }

    /// A job-log line written before nodes has no `node` and reads as "";
    /// a node from a daemon that sends fewer fields reads with the rest at
    /// their defaults.
    #[test]
    fn node_fields_default_when_absent() {
        let old = r#"{"id":3,"argv":["true"],"cwd":"/w/tree","repo":"repo","session":"",
            "client":"h:1","toolchain":"other","remote":false,"tokens":1,"state":"ok",
            "enqueued_ms":1,"started_ms":2,"ended_ms":3,"exit":0,"cpu_s":0.0,
            "compile_server_cpu_s":0.0,"peak_rss":0,"checkout":{"kind":"worktree","name":"fix"}}"#;
        let j: Job = serde_json::from_str(old).unwrap();
        assert_eq!(j.node, "");
        let n: Node = serde_json::from_str(r#"{"node_id":"local","cpus":8}"#).unwrap();
        assert_eq!(n.kind, NodeKind::Local);
        assert_eq!(n.state, NodeState::Ready);
        assert_eq!((n.cpus, n.slots, n.deadline.as_deref()), (8, 0, None));
        let remote: Node = serde_json::from_str(
            r#"{"node_id":"cloud-a","kind":"Remote","state":"Draining",
            "deadline":"2026-11-23T12:00:00Z","price_per_hour_cents":272}"#,
        )
        .unwrap();
        assert_eq!(
            (remote.kind, remote.state, remote.price_per_hour_cents),
            (NodeKind::Remote, NodeState::Draining, Some(272))
        );
        // The optional fields of the local node stay off the wire.
        let line = serde_json::to_string(&n).unwrap();
        assert!(
            !line.contains("deadline") && !line.contains("location"),
            "{line}"
        );
        assert!(line.contains(r#""kind":"Local","state":"Ready""#), "{line}");
        assert_eq!(rfc3339(1_763_899_200_000), "2025-11-23T12:00:00Z");
    }
}
