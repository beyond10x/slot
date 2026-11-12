---
format: aep.planning-md/3
id: story:pilot-session-measured
kind: story
status: draft
title: One pilot session builds through slot, measured against its builds before
relations:
- decomposes: epic:adoption-measured
- serves: vision:O6
- depends_on: story:permanent-service-unit
revision: 2
---
# Story: pilot-session-measured

## Outcome

One agent session runs every build through slot for at least one working day and at least 50
builds, and a verification-report in this store compares its builds on slot (from `jobs.jsonl`)
with the same session's builds before (from its session log), so the decision to widen adoption
rests on numbers.

## Context

- `jobs.jsonl` records per job: `session`, `toolchain`, `enqueued_ms`, `started_ms`, `ended_ms`,
  `exit`, `cpu_s`, `peak_rss` (`crates/slot/src/types.rs`, `Job`). Queue wait is
  `started_ms - enqueued_ms`; run time is `ended_ms - started_ms`.
- "Before" comes from the session log the way `docs/design.md` § 1 derived its numbers: wall time
  per build-class call. CPU seconds before slot are not in the session log; the report says so
  instead of inventing a number.
- The session is told by one line in its instructions to prefix builds with `slot`.
- Depends on `story:permanent-service-unit`.

## Acceptance

A verification-report in `.engineering/planning/` covers at least one working day and at least 50
jobs of the pilot session in `jobs.jsonl` (a report on fewer does not satisfy this story), and holds:
the session id; the `jobs.jsonl` filter used; sample sizes before and after; queue wait p50/p90, run time p50/p90 and CPU seconds p50/p90
per toolchain on slot; wall time p50/p90 for the same commands before; the count of builds the
session ran outside slot in the period; and a stated decision to continue or stop.

## Scope

- No source change expected. The report; a query script under `.engineering/` or `contrib/` if one
  is written.
