---
format: aep.planning-md/3
id: story:jobs-record-what-the-compile-cache-did
kind: story
status: draft
title: Jobs record what the compile cache did, and the cache counters are kept per minute
relations:
- serves: vision:O6
revision: 2
---
## Outcome

The job log and a counter log say how often the same Rust unit was compiled again: per job
what the compile cache did, and per minute what each cache server answered.

## Context

- Measured on 2026-11-22 on a 20-core workstation shared by about 15 agent sessions, over 28.4 h
  of one sccache 0.18.0 server: of 311,904 rustc calls, 26 % were answered from the cache, 16 %
  were compiled after a miss and 57 % were refused (incremental 24 %, clippy 19 %, binaries,
  tests and proc macros 14 %). The cacheable path used 27.7 CPU-hours, 11 % of the CPU the host
  used.
- On disk, 7,345 of 20,870 unit compilations (35 %) repeat a unit that another target directory
  built earlier. The job log has 13,018 compiling jobs in 4,456 target directories in 24 h, and
  5.5 % of those jobs ran in a directory that still exists, so the count is a lower bound.
- 66 % of the compile-server CPU the job log records (7.1 of 10.8 h in 24 h) falls in target
  directories younger than 30 minutes.
- None of this is recorded as it happens. sccache keeps global counters that reset at each
  server start and logs nothing per call without a restart. The job log has
  `compile_server_cpu_s` per job and nothing about what was compiled, with which rustc, or in
  which environment. The analysis took two sampling scripts and a scan of every target
  directory.
- A cache key of sccache holds the working directory, every `CARGO_*` variable, the rustc
  version and all arguments, so the environment of a job decides whether it can hit at all.
- An experiment on this repository: a new checkout with a warm cache compiled 101 units in
  64.6 s (80 hits, 14 misses); the same checkout with its target directory copied from a built
  one compiled 3 units in 15.4 s. A record per job is what shows whether such a change moves the
  numbers.

## Acceptance

1. Once a minute the daemon appends one record per running sccache server to a log in its state
   directory: time, port, hits, misses, refused calls by reason, cache size and limit. A counter
   that went down marks a server restart. No server is restarted or reconfigured for it.
2. A finished cargo job's record carries the rustc version and the values of
   `CARGO_INCREMENTAL`, `CARGO_PROFILE_*`, `RUSTFLAGS` and `RUSTC_WRAPPER` it ran with, and no
   other variable.
3. A finished job's record lists the units the compile server built for it: crate name,
   version, CPU seconds. The record says that it is a sample and names the interval.
4. `slot usage` has a mode that prints, for a window, the units compiled most often, with the
   number of target directories and the CPU seconds, and the hit rate per hour.
5. Every new field carries `#[serde(default)]`; an older client reads the newer daemon.

## Scope

- `crates/slot/src/procstat.rs`, `crates/slot/src/sccache.rs`, `crates/slot/src/server/mod.rs`,
  `crates/slot/src/types.rs`
- `crates/slot/src/usage.rs`, `crates/slot/src/cli.rs`, `docs/design.md` § 12
