---
format: aep.planning-md/3
id: story:memory-max-is-tested
kind: story
status: draft
title: memory-max kills only the job that exceeds it
relations:
- decomposes: epic:desktop-headroom
- serves: vision:O2
- depends_on: story:disk-guard
scope:
- confidence: inferred
  path: crates/slot/src/server/tests.rs
revision: 4
---
# Story: memory-max-is-tested

## Outcome

`slot serve --memory-max <limit>` is verified as it is built today: a job that exceeds the limit is
killed by the kernel inside its own scope, the client gets a non-zero exit, the job log records it,
and the daemon and every other job keep running. This story adds tests, not behaviour; a defect the
tests expose becomes its own story.

## Context

- The flag exists (`crates/slot/src/cli.rs`, `memory_max`; wrapper in
  `crates/slot/src/server/mod.rs`) but is off by default and untested (`docs/design.md` § 9 "Not
  verified", § 10).
- Needs a systemd user manager; the test is skipped with a stated reason where
  `systemd-run --user` is unavailable (CI containers).
- Shares `crates/slot/src/server/tests.rs` with the other headroom stories; ordered after
  `story:disk-guard` by `depends_on` for that file.

## Acceptance

1. A test starts a daemon with `--memory-max 64M`, runs a job that allocates 256 MiB and a second
   job that sleeps 2 s: the first exits non-zero, the second exits 0, and `GET /api/state` answers
   afterwards.
2. `jobs.jsonl` holds the first job with a non-zero `exit` and a `peak_rss` of at most 64 MiB
   (67108864 bytes).

## Scope

- `crates/slot/src/server/tests.rs`
