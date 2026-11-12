---
format: aep.planning-md/3
id: story:memory-max-scope-carries-cpu-weight
kind: story
status: draft
title: The per-job memory-max scope carries the same CPU weight
relations:
- decomposes: epic:desktop-headroom
- serves: vision:O2
- depends_on: story:builds-run-at-low-cpu-weight
- depends_on: story:memory-max-is-tested
scope:
- confidence: inferred
  path: crates/slot/src/server/mod.rs
- confidence: inferred
  path: crates/slot/src/server/tests.rs
revision: 3
---
# Story: memory-max-scope-carries-cpu-weight

## Outcome

When `slot serve --memory-max <limit>` wraps a job in `systemd-run --user --scope -p MemoryMax=…`,
the scope carries the same CPU weight as the daemon's unit (`story:builds-run-at-low-cpu-weight`),
so the memory limit does not move the job out from under the low weight.

## Context

- The wrapper argv is built in `crates/slot/src/server/mod.rs` (the `memory_max` branch before
  `look_path`); it passes `MemoryMax=` only. A transient scope started that way lands outside the
  daemon's cgroup at the default weight 100.
- Depends on `story:builds-run-at-low-cpu-weight` (one weight value) and on
  `story:memory-max-is-tested` (that story's tests cover the wrapper this one changes; both edit
  `crates/slot/src/server/tests.rs`).

## Acceptance

1. A unit test asserts that with `memory_max` set the job's argv contains `-p CPUWeight=20` beside
   `-p MemoryMax=<limit>`.
2. On a live daemon with `--memory-max 4G`, while a job runs,
   `systemctl --user show <the job's scope> -p CPUWeight` prints `CPUWeight=20`.

## Scope

- `crates/slot/src/server/mod.rs` (wrapper argv)
- `crates/slot/src/server/tests.rs`
