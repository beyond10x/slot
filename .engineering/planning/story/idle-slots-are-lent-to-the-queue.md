---
format: aep.planning-md/3
id: story:idle-slots-are-lent-to-the-queue
kind: story
status: implemented
title: A job that leaves its slots idle does not keep the queue waiting
relations:
- serves: vision:O2
scope:
- confidence: inferred
  path: AGENTS.md
- confidence: inferred
  path: CHANGELOG.md
- confidence: cited
  path: crates/slot/src/cli.rs
- confidence: cited
  path: crates/slot/src/jobserver.rs
- confidence: inferred
  path: crates/slot/src/server/handover.rs
- confidence: cited
  path: crates/slot/src/server/mod.rs
- confidence: cited
  path: crates/slot/src/server/tests.rs
- confidence: cited
  path: crates/slot/src/status.rs
- confidence: cited
  path: crates/slot/src/types.rs
- confidence: cited
  path: crates/slot/src/usage.rs
- confidence: cited
  path: docs/design.md
- confidence: inferred
  path: ui/src/generated/types.ts
- confidence: inferred
  path: website/docs/how-it-works.md
- confidence: inferred
  path: website/docs/reference/cli.md
revision: 18
transitions:
- {from: "draft", to: "proposed", at: "2026-11-22T18:26:43Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-22T18:26:43Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-23T01:34:03Z", actor: "human:timo", revision: 18, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome

A job that leaves its slots idle does not keep the queue waiting: while requests wait, slots of
running jobs that have used almost no CPU for some time are lent to the head of the queue, and
the pool's CPU use stays inside its limit.

## Context

- A job's slot count is fixed when it is enqueued (`server/mod.rs` `job_tokens`) and returned
  only when it ends (`finish`). The cores each job uses are sampled every second and only shown.
- Measured on one 20-CPU host, 29,442 job records over 5 active days: jobs that ran 10 s or more
  at under 0.25 core per slot held 131.4 of 450.4 slot-hours (29 %), 85.3 slot-hours of it while
  requests waited. The first-order avoidable wait (integral of the smaller of queued jobs and
  such slots) is 71.8 h of 265.7 h of queue wait; with under 0.1 core per slot, 38.3 h.
- By kind: 1,183 go jobs held 67.4 slot-hours and used 12.9 CPU-hours (0.19 core per slot);
  browser test runs held 38.9 slot-hours for 11.4 CPU-hours. In the worst hour 28.7 jobs were
  queued on average while 63 % of the slot-time belonged to jobs under 0.5 core per slot, which
  together used 1.24 cores.
- Lowering a toolchain's default is not the fix for go: `--go-tokens` K also sets
  `GOFLAGS -p=K` (`toolchain.rs`, `docs/design.md` § 8.3), so K = 1 runs the packages of a
  network-bound `go test ./...` one after another and holds 1 slot four times as long.
  § 8.3 reasons "a fixed K wastes little" from a 3 s median call; the slot-hours above come from
  the long calls.
- The pool already lends: `jobserver.rs` `lend` hands an urgent head a slot ahead of its turn.
  The controller of § 11.2 only shrinks the pool and is not enforced.
- The opposite case is rare on the same host: no queue, 3 or more slots free and CPU pressure of
  40 % or more in 2.1 % of census windows.

## Acceptance

1. Test, pool of 2 slots, idle threshold set to 2 s: two jobs that sleep hold both slots and a
   third request waits. The third job starts within 5 s while the first two still run, and
   `slot status` shows 2 slots lent.
2. Test: a lender that starts to use CPU again gets no further loan made against it, and the
   pool admits no new job until the slots of running jobs minus repaid loans are at or under
   the limit. A loan is repaid by the next job that ends.
3. Test: with lending switched off (`--lend-idle 0`, the value that keeps today's behaviour) the
   third job of case 1 does not start.
4. The loans outstanding at once are bounded by an option, and the default is written in
   `docs/design.md` with the measurement behind it.
5. The job record carries the slot-seconds a job lent; `slot usage` charges lent slot time to
   the job that ran on it.

## Scope

- `crates/slot/src/jobserver.rs` (`lend`, the queue), `crates/slot/src/server/mod.rs` (the
  sampler, `finish`, the job record), `crates/slot/src/server/tests.rs`
- `crates/slot/src/cli.rs`, `crates/slot/src/status.rs`, `crates/slot/src/usage.rs`,
  `crates/slot/src/types.rs`, `docs/design.md`, `website/docs/`
