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
#[command(name = "slot", version, args_conflicts_with_subcommands = true)]
pub struct Cli {
    /// Run the command here, without the daemon.
    #[arg(long, global = true)]
    pub local: bool,

    /// Subcommand, or the build command to run.
    #[command(subcommand)]
    pub command: Cmd,
}

/// Subcommands. Anything that is not one of them is a build command.
#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Run the daemon: the slot pool, the job queue and the web UI.
    Serve(ServeArgs),
    /// Bridge stdin/stdout to the daemon's socket (remote clients run this over ssh).
    Proxy(ProxyArgs),
    /// Run a build command; use this when the command's name is also a slot subcommand.
    Run(RunArgs),
    /// Show the pool and every queued and running job.
    Status(StatusArgs),
    /// Any other command is run as a build: `slot cargo test -p foo`.
    #[command(external_subcommand)]
    External(Vec<OsString>),
}

/// `slot serve` options.
#[derive(Debug, Clone, Args)]
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
    #[arg(long)]
    pub test_threads: Option<usize>,
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
}

/// `slot proxy` options.
#[derive(Debug, Clone, Args)]
pub struct ProxyArgs {
    /// The daemon's socket.
    #[arg(long, env = "SLOT_SOCKET", default_value_os_t = paths::default_socket())]
    pub socket: PathBuf,
}

/// `slot run` arguments.
#[derive(Debug, Clone, Args)]
pub struct RunArgs {
    /// The command and its arguments.
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

/// `slot status` options.
#[derive(Debug, Clone, Args)]
pub struct StatusArgs {
    /// The daemon's web address.
    #[arg(long, env = "SLOT_HTTP", default_value = "127.0.0.1:7411")]
    pub http: String,
}

fn default_tokens() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
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
        })
    }
}
