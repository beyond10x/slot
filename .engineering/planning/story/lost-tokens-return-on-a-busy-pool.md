---
format: aep.planning-md/3
id: story:lost-tokens-return-on-a-busy-pool
kind: story
status: draft
title: Lost jobserver tokens return within a minute on a busy pool
relations:
- serves: vision:O2
- informed_by: story:cancelled-job-leaves-no-process
revision: 1
---
## Outcome

A jobserver token that a killed process took and never wrote back is in circulation again at most
60 s after it was lost, also while other processes have the FIFO open. A token a live process
holds is never written back.

## Context

- `server/mod.rs` `token_watch` (0.12.0) restores lost tokens only when requests wait, no token is
  free, and `jobserver::holders` finds no process but the daemon with the FIFO open at two ticks
  15 s apart (`if !others.is_empty() { last = None; continue; }`). No other path returns a lost
  token on a pool that is not idle: `finish` releases the job's own slots, the idle reconcile
  needs `active` empty, the kill path in `supervise` writes nothing back, `sccache_watch` refills
  the sccache server's own pipe.
- Measured on one 20-CPU host, daemon 0.17.0, 27.9 h of census data: 4 `tokens_restored` events
  of 7, 2, 7 and 8 tokens. Each loss starts within 10 s of a job the daemon cancelled (three of
  them a `cargo` or `task` job cancelled 4 to 11 s after its start; the fourth within a 5-minute
  window of two cancels). The tokens stayed lost for 16.4, at most 23.7, 33.3 and 172.6 minutes.
  In the longest period 8 of 14 circulating tokens were gone and running jobs held 4.9 slots on
  average. Sum: 3.43 h with 2 or more tokens lost, 26.8 token-hours, 17.1 job-hours queued
  meanwhile.
- Why the repair waits: 49 of 49 live samples over about 2.5 minutes had 3 to 8 processes besides
  the daemon with the FIFO open (a `cargo` waiting on a test binary, a `cargo` whose children are
  sccache clients, `rustc`). The longest period holds 15 stretches of 30 s or more with requests
  waiting, no token free and under 0.02 compiler cores in pool jobs.
- A count that matched the pool: tokens not booked to a job (limit − free − slots of running
  jobs) equalled the sum over FIFO holders of (live direct children − 1) in 38 of 40 samples, and
  was 1 above it in 2. A holder waiting on one child holds no token. A holder with no compiler
  below it can hold tokens: a `cargo` with 2 sccache client children and no compiler in its
  subtree held 1 (15 observations). "Has a live compiler" is therefore not a safe test.
- 117 jobs were cancelled while running in the same log, against 4 restores and 5 idle
  reconciles: most cancels lose nothing.
- Related: `story:cancelled-job-leaves-no-process` changes how a cancelled group is killed. It
  does not return tokens.

## Acceptance

1. Test, pool of 6 tokens: a fixture takes 3 tokens from the FIFO and is killed with SIGKILL; a
   second fixture keeps the FIFO open, holds 1 token and has 2 live children; a request waits.
   Within 45 s the pool has written back 3 tokens, not 4, and the census log has one
   `tokens_restored` line with `tokens: 3`.
2. Test, same pool, nothing lost: two fixtures keep the FIFO open and hold 1 and 2 tokens with 2
   and 3 live children; a request waits for 60 s. The watch writes back 0 tokens.
3. Test: with 1 token more in circulation than the limit while jobs run, the surplus is taken out
   within 45 s. Today only the idle reconcile removes a surplus.
4. `slot status` prints the tokens that neither a job nor a live holder accounts for, and the
   age of the oldest such shortfall.
5. `docs/design.md` states the rule of the token watch and the two cases it must not confuse (a slow live
   holder, a holder whose children are compile-server clients).

## Scope

- `crates/slot/src/server/mod.rs` (`token_watch`), `crates/slot/src/jobserver.rs` (`holders`,
  `missing`, `reconcile`), `crates/slot/src/server/tests.rs`
- `crates/slot/src/status.rs`, `docs/design.md`
