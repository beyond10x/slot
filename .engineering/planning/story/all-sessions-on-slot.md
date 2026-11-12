---
format: aep.planning-md/3
id: story:all-sessions-on-slot
kind: story
status: draft
title: All agent sessions build through slot, measured
relations:
- decomposes: epic:adoption-measured
- serves: vision:O6
- depends_on: story:pilot-session-measured
revision: 2
---
# Story: all-sessions-on-slot

## Outcome

Every agent session on the workstation is instructed to build through slot (one line in the
workspace instructions, as `docs/design.md` § 4 proposes), and the effect is measured the way the
pilot was.

## Context

- Depends on `story:pilot-session-measured` deciding to continue.
- Not in this story: removing the per-session `flock` slot files and hand-set `CARGO_BUILD_JOBS`
  from workspace scripts (`docs/design.md` § 6 Phase 2). The daemon already drops client
  `CARGO_BUILD_JOBS` and `MAKEFLAGS` (`AGENTS.md` § Boundary), so a leftover cap does not bypass
  the pool.

## Acceptance

A verification-report in `.engineering/planning/` covers five consecutive working days after the
instruction lands and shows: every session that ran at least one build-class call in that period
(from the session logs) has at least one job in `jobs.jsonl`; at least 90 % of build-class calls in
the session logs appear in `jobs.jsonl`; and queue wait and run time p50/p90 per toolchain, with
the same-command p90/median ratio beside 5.2 (`docs/design.md` § 1). A report below either
threshold does not satisfy this story.

## Scope

- No change in this repository beyond the report. The workspace instructions file.
