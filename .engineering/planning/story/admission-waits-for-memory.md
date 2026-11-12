---
format: aep.planning-md/3
id: story:admission-waits-for-memory
kind: story
status: draft
title: Admission holds a job while available memory is below the reserve
relations:
- decomposes: epic:desktop-headroom
- serves: vision:O2
- depends_on: story:memory-max-scope-carries-cpu-weight
scope:
- confidence: inferred
  path: crates/slot/src/cli.rs
- confidence: inferred
  path: crates/slot/src/procstat.rs
- confidence: inferred
  path: crates/slot/src/server/mod.rs
- confidence: inferred
  path: crates/slot/src/server/tests.rs
- confidence: inferred
  path: crates/slot/src/types.rs
- confidence: inferred
  path: ui/src
- confidence: inferred
  path: website/docs/reference/cli.md
revision: 4
---
# Story: admission-waits-for-memory

## Outcome

Several link-heavy jobs at once no longer end in the kernel OOM killer: the daemon does not start
the next queued job while available memory (`MemAvailable` in `/proc/meminfo`) is below a
configured reserve (`slot serve --memory-reserve`), and starts it once memory is available again.
A held job shows as waiting with the reason `memory` in `GET /api/state` and in the UI.

## Context

- Today admission looks only at tokens (`crates/slot/src/server/mod.rs`); peak RSS is sampled per
  job (`crates/slot/src/procstat.rs`, `Job::peak_rss`) but not used to admit.
- Linking is the memory peak of a Rust build; several links at once is the OOM case.
- `--memory-max` (`story:memory-max-is-tested`) contains a job that does exceed; this story keeps
  jobs from starting into a shortage.
- Shares admission code with `story:disk-guard` and `server/mod.rs` / `server/tests.rs` with the
  other headroom stories; ordered after `story:memory-max-scope-carries-cpu-weight` by
  `depends_on` (the order is written out in `story:disk-guard`).

## Acceptance

1. A test with a fake meminfo source reporting less than the reserve: a queued job stays in state
   waiting with reason `memory` while a token is free; when the source reports enough, it starts
   within one admission tick. While it is held, `GET /api/state` lists it with state `queued` and
   reason `memory`.
2. A UI test (`task ui-check`) renders a queued job whose reason is `memory` with the text
   `memory` visible in its row.
3. With the reserve unset (default), admission is unchanged: existing tests
   `second_job_waits_for_the_slot` and `streams_output_and_exit_code` pass unmodified.
4. `slot serve --help` documents `--memory-reserve`; the generated CLI reference and UI types are
   regenerated.

## Scope

- `crates/slot/src/server/mod.rs` (admission)
- `crates/slot/src/procstat.rs` (meminfo reader)
- `crates/slot/src/cli.rs` (`ServeArgs`)
- `crates/slot/src/types.rs` (wait reason)
- `crates/slot/src/server/tests.rs`
- `ui/src/` (show the reason), `ui/src/generated/types.ts`, `website/docs/reference/cli.md`
