---
format: aep.planning-md/3
id: story:daemon-tests-wait-on-conditions
kind: story
status: active
title: No daemon test waits a fixed time, and the handover tests stay inside the 60 s bound
relations:
- serves: vision:O2
scope:
- confidence: cited
  path: AGENTS.md
- confidence: cited
  path: crates/slot-cli/tests/gate.rs
- confidence: cited
  path: crates/slot-cli/tests/handover.rs
- confidence: cited
  path: crates/slot/src/server/tests.rs
- confidence: inferred
  path: docs/design.md
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-11-22T18:26:42Z", actor: "agent:slot", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-22T18:26:42Z", actor: "agent:slot", revision: 3}
---
## Outcome

No test of the daemon waits a fixed time for something to happen, and the handover test binary
stays well inside the 60-second bound per test.

## Context

- `AGENTS.md` open item (s): `memory_short_holds_a_job_only_while_another_runs` expects a process
  300 ms after the submit, and `client_disconnect_kills_job_and_returns_token` expects the slot back
  300 ms after the process is gone (`crates/slot/src/server/tests.rs`). Each failed once in
  repeated runs on a workstation at load 59.
- Further tests that failed under load in repeated runs on 2026-11-22: `an_unreadable_handover_
  file_starts_a_plain_daemon`, `a_burst_of_reloads_never_ends_the_daemon`, and in
  `crates/slot-cli/tests/gate.rs` `a_gated_job_holds_its_slot_until_the_client_ends`.
- The handover test binary (`crates/slot-cli/tests/handover.rs`) took 50 to 64 s on 2026-11-22.
  Workspace rule: no single test over 60 s, the test step within 120 s.

## Acceptance

1. Every test named above waits on its condition with a deadline (a poll with a bound of at most
   10 s) instead of a fixed sleep; a search of the two test files finds no `sleep` that a test's
   assertion depends on, each remaining sleep carrying a comment saying why it is not a wait.
2. Each named test passes 20 of 20 runs while the host runs 20 busy loops (`yes > /dev/null`, one
   per CPU), run outside the pool; the runs are quoted in the pull request with their times.
3. The slowest test of the handover binary runs under 30 s on a host at load under 10, measured
   before and after the change and both times quoted.
4. `AGENTS.md` open item (s) is removed.
