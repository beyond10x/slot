---
format: aep.planning-md/3
id: story:jobs-ask-for-a-share-of-the-cpus
kind: story
status: draft
title: A job asks for a share of the CPUs, and the pool admits while that share is free
summary: admission by CPU share between two bounds, no fixed count of slots
relations:
- serves: vision:O2
- informed_by: story:idle-slots-are-lent-to-the-queue
- informed_by: story:compile-servers-count-as-pool-load
revision: 1
---
## Outcome

A job asks the pool for a share of the CPUs, not for a number of slots, and the daemon admits
the head of the queue while that share is free: the number of running jobs follows the CPU the
jobs use, between a lower and an upper bound, and is no fixed count.

## Why

The pool hands out slots on the assumption that one slot keeps one CPU busy. Measured on one host
of 20 CPUs, 2026-11-22:

- Over 3 hours a slot used 0.90 CPUs on average; a quarter of the slot-hours went to jobs under
  0.25 CPUs per slot, 12.3 % to jobs at 1.5 and more.
- 256 jobs whose command was a script held 60 % of the slot-hours at 0.74 CPUs per slot.
- With 17 running slots: every slot taken, 59 jobs queued, the longest wait 31 minutes, and the
  pool's cgroup at 7.8 of its 17 CPUs.
- With 50 running slots (`--tokens 59`, headroom 15 %, the same CPU quota): 12.3 jobs a minute
  ended against 7.7, the 90th percentile of the queue wait was 97 s against 1,467 s, the host
  was at 20.0 of 20 CPUs, tasks of the pool waited for a CPU 68 % of the time, and single jobs
  ran longer (`pnpm build` 8 s, then 120 s).
- One sample with 50 slots: 30 running jobs held 42 slots by their requests, 0 were free, and
  the jobs used 12.3 CPUs.
- The compilers of a rust job run in the compile cache server, outside the job's cgroup: 8.9
  CPUs in one 15 s sample.

Neither number is right: 17 leaves CPUs idle while jobs wait, 50 starts jobs that then wait for
a CPU.

## Design, the operator's words in short

- "It doesn't need to be a fixed amount": the daemon reads the CPUs of the host and what the
  pool uses, and takes a job from the queue while a share is available for it ("every compile
  could get 0.4 cores, or something like that"); otherwise it takes none.
- "Two boundaries on each side": a lower and an upper bound on the number of running jobs.
- "Jobs should not request number of slots, but shares of cores."

## What exists

- `policy::Controller` computes a pool size from the census every tick; it is shown
  (`recommended_tokens`, the pool chart) and not applied.
- A job's slot count is fixed at enqueue (`job_tokens`) and returned when the job ends.
- Each job's CPU use is sampled every second (`cpu_s`, the cores column) and the job log keeps,
  per repository and command, what earlier runs used.
- The jobserver FIFO bounds what `cargo` and `make` start inside a job; it counts whole tokens.

## Open questions

- What a job's share is when it asks for none: the measured share of earlier runs of the same
  repository and command, or a default per toolchain.
- How the share of a rust job counts the compilers that run in the compile cache server.
- Whether the jobserver FIFO keeps bounding parallelism inside a job, with admission moved to
  shares, or goes with it.
- What `--tokens N` and the `--test-threads` share mean afterwards.
- The two bounds and the free share that admits a job: fixed settings or derived from the host.

## Acceptance

- With jobs that each use 0.25 CPUs, the daemon runs more of them at once than the host has
  CPUs, up to the upper bound; with jobs that each use 4 CPUs, fewer than a quarter of the CPUs.
- A queued job starts within one tick of the free share reaching the admission share, and none
  starts below it, except to keep the lower bound.
- The pool's CPU use stays inside its quota, and `slot status` and the web view show the share
  each job asked for, the share it uses and the free share.
- A client that asks for slots (0.21.0 and older) still runs: its request is read as a share.

## Related

`story:idle-slots-are-lent-to-the-queue` and `story:compile-servers-count-as-pool-load` are
cases of this change.
