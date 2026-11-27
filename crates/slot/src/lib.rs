//! slot: one shared pool of build slots for every build on a machine.
//!
//! `slot serve` owns a GNU-make jobserver FIFO with one token per CPU slot and
//! queues every job on it. `slot <command…>` sends a command to the daemon,
//! streams its output back and exits with its code. In remote mode the client
//! syncs the repository to a build server over ssh first.

pub mod admission;
pub mod allow_origins;
pub mod artifacts;
pub mod build_version;
pub mod call;
pub mod cargo_fetch;
pub mod cargo_lock;
pub mod census;
pub mod cli;
pub mod client;
pub mod cpu_limit;
pub mod evict;
pub mod extra_trees;
pub mod gate;
pub mod git_deps;
pub mod hook;
pub mod hostconfig;
pub mod job_parallel;
pub mod jobserver;
pub mod mirror;
pub mod never_sync;
pub mod paths;
pub mod phases;
pub mod policy;
pub mod procstat;
pub mod project;
pub mod proto;
pub mod refused;
pub mod repo_config;
pub mod return_sync;
pub mod rewrite;
pub mod run_limit;
pub mod same_path;
pub mod sandbox;
pub mod sccache;
pub mod server;
pub mod sessions;
pub mod shares;
pub mod shell;
pub mod status;
pub mod sync;
pub mod toolchain;
pub mod types;
pub mod usage;
pub mod width;
