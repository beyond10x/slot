---
format: aep.planning-md/3
id: story:short-jobs-start-ahead-of-long-ones
kind: story
status: draft
title: A job predicted to run a few seconds starts ahead of longer jobs of its level
relations:
- serves: vision:O2
revision: 1
---
## Outcome

Within one priority level, a waiting job that is predicted to run a few seconds starts ahead of
jobs predicted to run longer, and a job passed over this way still starts within a stated bound.

## Context

- The queue is ordered by priority level, then arrival (`jobserver.rs` `WaiterState::rank`).
- The daemon already predicts a command's run time: `admission.rs` `History::expected_runtime`,
  the 90th percentile of the last 20 runs of the same repository and argv that ended on their
  own, with at least 3. Its only use is the upgrade hold.
- `docs/design.md` § 11.3 lists shortest-predicted-first as an open question for the job log.
- Measured on one 20-CPU host, the busiest day of the log (17,682 started jobs): 72 % of the
  jobs ran under 5 s and carried 47.4 h of the day's 94.8 h of queue wait. 2,246 jobs waited
  more than 10 times their run time: 64.2 h of wait for 1.24 h of running.
- Replay of that day with the daemon's own predictor: 7,944 jobs (44.7 %) were predicted at 5 s
  or less; they waited 22.6 h and needed 1.8 slot-hours. 7 of them (0.1 %) then ran over 30 s.
  44.5 % of all jobs have fewer than 3 earlier runs, so the predictor has no value for them.
- A `cargo test` whose build is up to date ran 0.6 s at the median that day and waited 28 s at
  the 90th percentile and 4.2 minutes at the 99th.

## Acceptance

1. Test, pool of 1 slot, all requests at one level: a request with no history waits, then a
   request whose history predicts 5 s or less arrives. When the slot frees, the second request
   starts first.
2. Test: a request passed over is not passed again once it has waited the bound (an option,
   default written in `docs/design.md`); a stream of short requests does not hold it past that
   bound.
3. The order between priority levels is unchanged: a short request of a lower level does not
   pass a request of a higher level (test).
4. The job record carries the prediction (`predicted_ms`, absent without history), so the share
   of jobs predicted short that ran over 30 s can be read from the log; `slot status` marks a
   request that was moved ahead.
5. With the option off the order is priority then arrival, as today (test).

## Scope

- `crates/slot/src/jobserver.rs` (`rank`, the queue), `crates/slot/src/server/priority.rs`,
  `crates/slot/src/admission.rs`, `crates/slot/src/server/mod.rs` (enqueue, the job record),
  `crates/slot/src/server/tests.rs`
- `crates/slot/src/cli.rs`, `crates/slot/src/status.rs`, `crates/slot/src/types.rs`,
  `docs/design.md`
