//! Types the daemon sends to the web UI. TypeScript copies in
//! `ui/src/generated/` are generated from these by `slot-docs ts-types` and
//! drift-checked by `task check`.

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

/// One job, as logged and as shown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
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

/// Snapshot served at `GET /api/state`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct State {
    /// Host name.
    pub host: String,
    /// CPUs on the host.
    pub cores: u32,
    /// Pool size.
    pub tokens: u32,
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
    },
}

/// One chunk on `GET /api/jobs/{id}/output`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct Chunk {
    /// `out` or `err`.
    pub t: String,
    /// Text.
    pub b: String,
}
