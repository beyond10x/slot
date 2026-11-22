---
format: aep.planning-md/3
id: story:a-whole-pool-request-does-not-stall-the-queue
kind: story
status: implemented
title: A job that asks for every slot does not keep freed slots idle while others wait
relations:
- serves: vision:O2
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: cited
  path: crates/slot/src/jobserver.rs
- confidence: cited
  path: crates/slot/src/server/mod.rs
- confidence: inferred
  path: crates/slot/src/server/tests.rs
- confidence: cited
  path: crates/slot/src/status.rs
- confidence: inferred
  path: crates/slot/src/types.rs
- confidence: cited
  path: docs/design.md
- confidence: cited
  path: ui/src/components/QueueTable.vue
- confidence: inferred
  path: ui/src/generated/types.ts
- confidence: inferred
  path: ui/ux/scenarios.ts
- confidence: cited
  path: website/docs/how-it-works.md
revision: 15
transitions:
- {from: "draft", to: "proposed", at: "2026-11-22T18:26:44Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-22T18:26:44Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-22T20:50:28Z", actor: "human:timo", revision: 15, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome

A job that asks for every slot in circulation does not keep freed slots idle while other jobs
wait behind it.

## Context

- The pool's reader thread gives every token that returns to the FIFO to the first waiting job
  (`crates/slot/src/jobserver.rs`, the loop that adds to `got`). A job that asks for N slots
  collects them one by one and starts when it holds N.
- Measured on 2026-11-22 on a 20-core host with 17 slots in circulation: a job submitted with
  `--tokens 17` stood first in the queue for about 13 minutes. It had collected 11 slots; 5
  running jobs held 6, and 3 were held back as headroom. 54 jobs waited behind it, 53 of them
  for 1 slot each. One of the running jobs had been running for over an hour, so the 17-slot job
  could not start.
- `slot status` read "0 free, 5 running, 54 queued". No line named the 11 collected slots, and
  the state was reported as lost tokens. The lost-token check was right: it counts collected
  slots as booked.
- After the job's owner cancelled it, 17 jobs ran within minutes.
- Related: `story:idle-slots-are-lent-to-the-queue`, `story:short-jobs-start-ahead-of-long-ones`.

## Acceptance

1. A test with 4 slots: one running job holds 1 slot and does not end, a queued job asks for 4,
   three 1-slot jobs queue behind it. The three start within 1 s. Today none starts.
2. The job that asks for the whole pool still starts: the design names the bound after which no
   new job is admitted ahead of it, and a test covers that bound.
3. `slot status` and the web view name the slots a waiting job has collected, for example
   "#30036 holds 11 of 17".
4. The lost-token check still counts collected slots as booked.

## Scope

- `crates/slot/src/jobserver.rs`, `crates/slot/src/server/mod.rs`, `crates/slot/src/status.rs`
- `ui/src/components/QueueTable.vue`, `docs/design.md`
