---
format: aep.planning-md/3
id: story:jobs-run-on-a-cpu-set-of-their-share
kind: story
status: draft
title: Each job runs on a CPU set of its share, so load stays at or under the CPU count
relations:
- decomposes: epic:desktop-headroom
- serves: vision:O2
- informed_by: story:test-execution-bounded-by-pool
revision: 2
---
# Story: jobs-run-on-a-cpu-set-of-their-share

## Outcome

With the pool full, the host's 1-minute load stays at or under its CPU count. Each running job is
confined to a CPU set the size of its share, so a test runner, a compiler front-end or any tool
that sizes itself by `num-cpus` (`nproc`, `std::thread::available_parallelism`, nextest
`test-threads = "num-cpus"`) reads the job's share, not the host's CPU count.

## Evidence (2026-11-24, a 64-CPU build node, 51 slots, headroom 20 %)

Full pool of 51 low-priority jobs, each running 4 busy threads for 60 s, load sampled every 10 s:

| admission setting | peak load1 | note |
|---|---|---|
| CPU limit 51, pressure 60 % | 99.60 | 44 jobs started within the first 10 s, before their CPU use was measured |
| CPU limit 51, pressure 5 % | 84.38 | mean 59.5; 30 of 60 samples over 64 |
| CPU limit 51, pressure 60 %, `CPUQuota` 51 CPUs on the daemon's unit and 13 on the compile cache server's | 103.15 | throttled threads still count in the load average; quota removed |

Real jobs alone, no synthetic load: load1 88 with 13 jobs running and 34 slots free (rustc
front-ends, test runners at the host's CPU count).

Admission bounds how many jobs start. It cannot bound the threads a job starts after it runs.

## Recommendation

- At start, the daemon places a job in a transient scope (it already does for `--memory-max`)
  with `AllowedCPUs=` a set of N CPUs, N = the slots the job holds (`--tokens`), at least 1. Under
  `admission shares` N is the job's predicted share.
- CPU sets are allocated from a free list so running jobs do not overlap; a job that cannot get a
  disjoint set waits like a job that cannot get a slot.
- The compile cache server keeps the CPUs no job holds (its compilers run for jobs whose own set is
  full).
- `slot status` and the job log show each job's CPU set.
- `RUST_TEST_THREADS` and `NEXTEST_TEST_THREADS` follow N (replaces the free-token count of
  `story:test-execution-bounded-by-pool`, which this story supersedes for the thread bound).

## Acceptance

- A test runs 51 jobs of 4 busy threads on a pool of 51 slots on a 64-CPU host, and load1 sampled
  every 10 s stays at or under 64 for the whole run.
- Inside a job holding 4 slots, `nproc` prints 4.
- A job of 1 slot running `cargo nextest run` starts 1 test process at a time.
- A job's CPU set is the same size on this host and on a build node for the same request, so a
  test's thread count does not depend on where it ran.
