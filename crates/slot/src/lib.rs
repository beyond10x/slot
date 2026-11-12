//! slot: one shared pool of build slots for every build on a machine.
//!
//! `slot serve` owns a GNU-make jobserver FIFO with one token per CPU slot and
//! queues every job on it. `slot <command…>` sends a command to the daemon,
//! streams its output back and exits with its code. In remote mode the client
//! syncs the repository to a build server over ssh first.

pub mod census;
pub mod cli;
pub mod client;
pub mod jobserver;
pub mod paths;
pub mod policy;
pub mod procstat;
pub mod proto;
pub mod rewrite;
pub mod server;
pub mod toolchain;
pub mod types;
