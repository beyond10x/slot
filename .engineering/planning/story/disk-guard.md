---
format: aep.planning-md/3
id: story:disk-guard
kind: story
status: draft
title: A build does not start below the configured free disk space
relations:
- decomposes: epic:desktop-headroom
- serves: vision:O2
- depends_on: story:test-execution-bounded-by-pool
scope:
- confidence: inferred
  path: crates/slot/src/cli.rs
- confidence: inferred
  path: crates/slot/src/server/metrics.rs
- confidence: inferred
  path: crates/slot/src/server/mod.rs
- confidence: inferred
  path: crates/slot/src/server/tests.rs
- confidence: inferred
  path: crates/slot/src/types.rs
- confidence: inferred
  path: ui/src/generated/types.ts
- confidence: inferred
  path: website/docs/reference/cli.md
revision: 5
---
# Story: disk-guard

## Outcome

The daemon refuses to start a build when the filesystem holding that build's output has less free
space than a configured minimum (`slot serve --min-free-disk`, default `25G`, the organization
rule). A refused job ends at once with a non-zero exit, one stderr line naming the filesystem, the
free space and the minimum, and a job-log line in a refused state; jobs on filesystems with enough
space run as before.

## Context

- Nothing checks disk before a build today; `docs/design.md` § 1 names disk as the second
  constraint on the workstation.
- The build output's filesystem: `CARGO_TARGET_DIR` (or `GOCACHE` for go) when set in the job's
  environment, the per-workspace cache directory in remote mode, else the job's working directory.
- Shares admission in `crates/slot/src/server/mod.rs`, `ServeArgs`, `types.rs`,
  `server/tests.rs` and the generated `cli.md` / `types.ts` with `story:admission-waits-for-memory`,
  and `server/mod.rs` / `server/tests.rs` with the other headroom stories. Order on those files:
  `story:test-execution-bounded-by-pool` → this story → `story:memory-max-is-tested` →
  `story:memory-max-scope-carries-cpu-weight` → `story:admission-waits-for-memory` →
  `story:prometheus-metrics` → `story:path-shim`, each a `depends_on` on the one before.

## Acceptance

1. A test starts a daemon with `--min-free-disk` set above the free space of the tempdir's
   filesystem: the job exits non-zero without running its command (a marker file it would write
   does not exist), stderr names the filesystem, free bytes and minimum, and `jobs.jsonl` records
   the job as refused.
2. The same test with a minimum below the free space runs the job and exits 0.
3. `slot serve --help` documents `--min-free-disk` and its default; the generated CLI reference is
   regenerated (`task generate`).

## Scope

- `crates/slot/src/cli.rs` (`ServeArgs`)
- `crates/slot/src/server/mod.rs` (admission)
- `crates/slot/src/types.rs` (job state, if a refused state is added)
- `crates/slot/src/server/metrics.rs` (its exhaustive `match` on `JobState` maps a refused job to a
  `result` label; shared with `story:prometheus-metrics`, which follows on the chain)
- `crates/slot/src/server/tests.rs`
- `website/docs/reference/cli.md`, `ui/src/generated/types.ts` (generated)
