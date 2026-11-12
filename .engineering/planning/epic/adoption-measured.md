---
format: aep.planning-md/3
id: epic:adoption-measured
kind: epic
status: draft
title: Every build on the operator's workstation goes through slot, measured
summary: Permanent service, one pilot session measured, then all sessions, then a PATH shim for cargo and go.
relations:
- serves: vision:O6
- informed_by: specification:delivered-baseline
revision: 1
---
# Epic: every build on the operator's workstation goes through slot, measured

## Outcome

Every agent session on the operator's workstation builds through slot, reached in measured steps:
a permanent service that survives reboot, one pilot session measured against its own builds
before, then the remaining sessions, then a `PATH` shim for `cargo` and `go` so adoption no longer
depends on each session following an instruction.

## Why

O6: the before/after is the point. `docs/design.md` § 1 measured contention from 38 days of
session logs (same command on the same package: p90/median 5.2; 1,648 calls behind a `flock` slot
file, median 80 s); § 6 Phase 0 sets the exit criterion as two weeks of job log compared with that.
Each step is re-measured from `jobs.jsonl` before the next one starts.

## Stories

1. `story:permanent-service-unit` — `task service` installs the unit; it replaces the transient
   unit and survives reboot.
2. `story:pilot-session-measured` — one session on slot, measured against its builds before.
3. `story:all-sessions-on-slot` — the remaining sessions, measured.
4. `story:path-shim` — `cargo` and `go` on `PATH` route through `slot`.

Order: 1 → 2 → 3 → 4.

## Not in this epic

Remote mode (`story:remote-mode-on-a-real-server`). Other machines than the operator's workstation.

## Acceptance

Done when a verification-report in this store shows, for five consecutive working days after the
shim is installed, that at least 90 % of build-class calls found in the session logs appear in
`jobs.jsonl`, with queue wait p50/p90 and run time p50/p90 beside the pilot's before-numbers.
