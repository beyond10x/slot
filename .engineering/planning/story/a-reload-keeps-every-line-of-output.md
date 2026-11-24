---
format: aep.planning-md/3
id: story:a-reload-keeps-every-line-of-output
kind: story
status: implemented
title: A reload loses no line of a running job's output, also under load
relations:
- serves: vision:O2
scope:
- confidence: cited
  path: AGENTS.md
- confidence: inferred
  path: CHANGELOG.md
- confidence: cited
  path: crates/slot-cli/tests/handover.rs
- confidence: cited
  path: crates/slot/src/server/handover.rs
- confidence: cited
  path: crates/slot/src/server/mod.rs
- confidence: inferred
  path: crates/slot/src/server/tests.rs
- confidence: inferred
  path: docs/design.md
revision: 11
transitions:
- {from: "draft", to: "proposed", at: "2026-11-22T18:26:43Z", actor: "agent:slot", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-22T18:26:43Z", actor: "agent:slot", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-22T20:50:27Z", actor: "agent:slot", revision: 11, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome

A reload of the daemon loses no line of a running job's output, on a loaded host as on an idle
one.

## Context

- `AGENTS.md` open item (h): at a load of 98 to 123 on 20 CPUs,
  `reloads_keep_running_and_queued_jobs_their_output_and_slots` failed in 1 of 6 runs on 0.21.1
  and in 2 of 6 with the run-time limit.
- Not verified: on a handover `client_writer` answers the request for unsent bytes with `pending`
  only, and frames still in its channel are dropped (`crates/slot/src/server/handover.rs`,
  `crates/slot/src/server/mod.rs`).

## Acceptance

1. The cause is shown first: a test that fails on `main` by putting frames into a job's channel
   right before the handover and reading the client's output after it.
2. With the change that test passes, and
   `reloads_keep_running_and_queued_jobs_their_output_and_slots` passes 20 of 20 runs while the
   host runs 20 busy loops (`yes > /dev/null`), run outside the pool.
3. The handover state's `VERSION` changes only if the state file's shape changes; a release that
   changes it reads the old one (`server/handover.rs`).
4. `AGENTS.md` open item (h) is removed.
