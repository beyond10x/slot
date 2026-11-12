---
format: aep.planning-md/3
id: story:prometheus-metrics
kind: story
status: draft
title: The daemon serves Prometheus metrics at GET /metrics
relations:
- serves: vision:O6
- informed_by: specification:delivered-baseline
- depends_on: story:admission-waits-for-memory
scope:
- confidence: inferred
  path: crates/slot/src/server/http.rs
- confidence: inferred
  path: crates/slot/src/server/metrics.rs
- confidence: inferred
  path: crates/slot/src/server/mod.rs
- confidence: inferred
  path: crates/slot/src/server/tests.rs
- confidence: inferred
  path: website/docs/metrics.md
- confidence: inferred
  path: website/sidebars.ts
revision: 6
---
# Story: prometheus-metrics

## Outcome

The daemon answers `GET /metrics` on its HTTP address in the Prometheus text exposition format
(version 0.0.4), so pool contention can be graphed and alerted on next to the rest of the machine:

| metric | type | labels |
|---|---|---|
| `slot_tokens` | gauge | — |
| `slot_tokens_free` | gauge | — |
| `slot_jobs_queued` | gauge | `toolchain` |
| `slot_jobs_running` | gauge | `toolchain` |
| `slot_jobs_finished_total` | counter | `toolchain`, `result` (`ok`, `failed`, `cancelled`) |
| `slot_job_wait_seconds` | histogram | `toolchain` |
| `slot_job_run_seconds` | histogram | `toolchain` |
| `slot_job_cpu_seconds_total` | counter | `toolchain` |

## Context

- **Already in the working tree at adoption, uncommitted, written by another session while this
  store was drafted:** `crates/slot/src/server/metrics.rs` (`Histogram`, `Metrics::record`,
  `render`, test `renders_counters_and_histograms`), the route `.route("/metrics", get(metrics))`
  in `crates/slot/src/server/http.rs`, and calls to `metrics.record` / `metrics_text` in
  `crates/slot/src/server/mod.rs`. The names and labels above are the ones that code uses; this
  story records that work and is what it is accepted against, not a second design.
- The `result` label follows `JobState` (`crates/slot/src/types.rs`): `Ok` → `ok`, `Failed` →
  `failed`, `Cancelled` → `cancelled`. A job refused by `story:disk-guard` and a job held by
  `story:admission-waits-for-memory` have to map onto these labels when those stories land.
- Counters start at zero when the daemon starts; the job log stays the durable record.
- Ordered after `story:admission-waits-for-memory` by `depends_on` for later changes to
  `server/mod.rs`, `server/tests.rs` and `server/metrics.rs`; the code already in the tree predates
  that order and is reconciled when this story is accepted.

## Acceptance

1. A test runs two jobs on a 2-token daemon, one exiting 0 and one exiting 3, then fetches
   `/metrics` over HTTP: `Content-Type` starts with `text/plain; version=0.0.4`, the body contains
   `slot_tokens 2`, `slot_jobs_finished_total{toolchain="other",result="ok"} 1`,
   `slot_jobs_finished_total{toolchain="other",result="failed"} 1` and
   `slot_job_run_seconds_count{toolchain="other"} 2`.
2. The fetched body passes `promtool check metrics` (or an exposition-format parser in the test)
   with no error.
3. While a job waits on a full pool, `slot_jobs_queued{toolchain="other"}` is 1.

## Scope

- `crates/slot/src/server/metrics.rs`
- `crates/slot/src/server/http.rs` (route)
- `crates/slot/src/server/mod.rs` (recording)
- `crates/slot/src/server/tests.rs`
- `website/docs/metrics.md` (new), `website/sidebars.ts` (lists pages by name)
