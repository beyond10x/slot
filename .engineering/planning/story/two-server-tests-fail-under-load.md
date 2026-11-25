---
format: aep.planning-md/3
id: story:two-server-tests-fail-under-load
kind: story
status: draft
title: Two server tests fail under load and pass alone
relations:
- serves: vision:O6
revision: 1
---
# Two server tests fail under load and pass alone

## Why

On 2026-11-25 two tests of `crates/slot/src/server/tests.rs` each failed once in a full
`cargo test -p b10x-slot --lib` run on a loaded pool and passed every time after, alone and in
full runs. Neither is in code the changes of that night touched.

- `the_eviction_mark_is_set_at_run_time_saved_and_looked_at_once`: the eviction report was read
  while still empty (`left: (0, 0)`, `right: (50000000000, 0)`); that run took 87 s against
  9 to 28 s for the others.
- `by_share_jobs_of_four_cpus_run_fewer_than_a_quarter_of_the_cpus`: two jobs let through where
  the test expects three (`left: [2, 3]`, `right: [2, 3, 4]`); alone it takes 4.0 s.

A test that fails on a loaded host fails the gate of whoever is merging at that moment.

## Acceptance

- Each test waits for the state it asserts (the report written, the jobs admitted) by a
  condition with a bound, not by a fixed sleep or the first read.
- Red first: each test run 20 times in a row under `stress`-like CPU load (or with the pool's
  admission tick slowed in the test) fails at least once before the change and never after.
- Neither test over 5 s.
