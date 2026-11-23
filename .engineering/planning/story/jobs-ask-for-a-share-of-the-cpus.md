---
format: aep.planning-md/3
id: story:jobs-ask-for-a-share-of-the-cpus
kind: story
status: implemented
title: A job asks for a share of the CPUs, and the pool admits while that share is free
summary: admission by CPU share between two bounds, no fixed count of slots
relations:
- serves: vision:O2
- informed_by: story:idle-slots-are-lent-to-the-queue
- informed_by: story:compile-servers-count-as-pool-load
scope:
- confidence: cited
  path: crates/slot/src/admission.rs
- confidence: cited
  path: crates/slot/src/census.rs
- confidence: cited
  path: crates/slot/src/cli.rs
- confidence: inferred
  path: crates/slot/src/client.rs
- confidence: inferred
  path: crates/slot/src/cpu_limit.rs
- confidence: inferred
  path: crates/slot/src/job_parallel.rs
- confidence: cited
  path: crates/slot/src/jobserver.rs
- confidence: cited
  path: crates/slot/src/policy.rs
- confidence: cited
  path: crates/slot/src/proto.rs
- confidence: inferred
  path: crates/slot/src/sccache.rs
- confidence: inferred
  path: crates/slot/src/server/handover.rs
- confidence: inferred
  path: crates/slot/src/server/limits.rs
- confidence: inferred
  path: crates/slot/src/server/machine.rs
- confidence: cited
  path: crates/slot/src/server/mod.rs
- confidence: inferred
  path: crates/slot/src/server/tests.rs
- confidence: cited
  path: crates/slot/src/status.rs
- confidence: cited
  path: crates/slot/src/toolchain.rs
- confidence: cited
  path: crates/slot/src/types.rs
- confidence: inferred
  path: docs/design.md
- confidence: inferred
  path: ui/src/components/ActiveTable.vue
- confidence: inferred
  path: ui/src/components/QueueTable.vue
- confidence: inferred
  path: ui/src/generated/types.ts
- confidence: inferred
  path: ui/ux/scenarios.ts
- confidence: inferred
  path: website/docs/how-it-works.md
- confidence: inferred
  path: website/docs/reference/cli.md
revision: 31
transitions:
- {from: "draft", to: "proposed", at: "2026-11-22T18:26:43Z", actor: "human:timo", revision: 4}
- {from: "proposed", to: "active", at: "2026-11-22T18:26:43Z", actor: "human:timo", revision: 5}
- {from: "active", to: "implemented", at: "2026-11-23T01:34:03Z", actor: "human:timo", revision: 31, decided_on: {"recorded":{"test_result":1}}}
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

## Research, 2026-11-22

Read on the pages by a research pass; values marked (start) are starting values of this story,
not taken from a source.

Policy:

- Shortest-remaining-time-first gives the lowest mean response time, and with heavy-tailed job
  sizes it costs the long jobs little: <https://www.cs.cmu.edu/~harchol/Papers/Sigmetrics01.pdf>.
- With sizes not known, least-attained-service is the best size-blind order when sizes have a
  decreasing failure rate: <https://arxiv.org/html/math/0412182>. A multi-level feedback queue
  is its practical form: <https://pages.cs.wisc.edu/~remzi/OSTEP/cpu-sched-mlfq.pdf>.
- A limit on running jobs is a first-come queue in front of a shared processor. A low limit acts
  like first-come-first-served, so mean response time grows with the spread of job sizes; the
  limit is tuned by a feedback loop: <https://www.cs.cmu.edu/~harchol/Papers/icde06.pdf>.
- When the submitters wait for their jobs (a closed system), queue order gains less than in an
  open one:
  <https://www.usenix.org/legacy/events/nsdi06/tech/full_papers/schroeder/schroeder_html/index.html>.

Build systems:

- GNU make `-l` starts no job while the load is over a limit; 3.81 fixed a burst of starts and
  4.4 replaced the load average by the runnable count:
  <https://www.gnu.org/software/make/manual/html_node/Parallel.html>.
- Bazel admits local actions by declared resources and has a switch, off by default, that admits
  by measured CPU load over a window instead of per-action estimates
  (`--experimental_cpu_load_scheduling` in
  <https://raw.githubusercontent.com/bazelbuild/bazel/master/src/main/java/com/google/devtools/build/lib/exec/ExecutionOptions.java>).
- Portage pairs `--jobs` with `--load-average` and calls the load average "a damped value":
  <https://wiki.gentoo.org/wiki/MAKEOPTS>. Ninja, Nix, Pants and GitLab Runner use fixed counts.

Signals:

- Pressure stall information: "some" is the share of time at least one task is stalled; its
  known users act on memory pressure, not CPU: <https://docs.kernel.org/accounting/psi.html>.
  No source gives a target for CPU pressure or for runnable tasks per CPU.
- The load average counts tasks in uninterruptible sleep and lags a minute:
  <https://www.brendangregg.com/blog/2017-08-08/linux-load-averages.html>.
- Adaptive concurrency limits of services compare a sample latency with the best seen and move
  the limit by a step per window: <https://github.com/Netflix/concurrency-limits>,
  <https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/adaptive_concurrency_filter>.

Shares:

- Kubernetes takes requests under one CPU; the request is a reservation at admission and a
  weight under contention:
  <https://kubernetes.io/docs/concepts/configuration/manage-resources-containers/>.
- Estimates from history: samples weighted by age (half-life 12 h for CPU), the weighted mean
  for batch jobs, a high percentile for serving jobs, then a margin of 10 to 15 %:
  <https://john.e-wilkes.com/papers/2020-EuroSys-Autopilot.pdf>.
- `cpu.weight` shares CPU among competing groups and bounds neither runnable tasks nor memory:
  <https://docs.kernel.org/admin-guide/cgroup-v2.html>.

## Design that follows

1. The objective is the slowdown of a job: its wall time over the best earlier wall time of the
   same repository and command. Not jobs per minute.
2. A job's share is predicted from the job log: CPU seconds over wall time of earlier runs,
   weighted by age, a high percentile plus 15 %, at least 0.1; an unknown command counts 1.0
   (start).
3. The daemon admits the head of the queue while the CPUs busy over the last 5 s (the pool's
   cgroup and the compile cache server), plus the shares of jobs started in those 5 s, plus the
   candidate's share, stay within a budget.
4. The budget starts at 15 CPUs on a host of 20 (the level measured busy with 17 slots) and
   never passes the pool's CPU quota. Every 5 s: median slowdown of finished jobs under 1.3
   raises it by 0.5; over 2 multiplies it by 0.85 (start).
5. CPU pressure of the host is a brake, not the target: over a threshold taken from the log,
   only short jobs start.
6. Queue order: shortest predicted wall time first; waiting lowers a job's key, so a long job
   starts.
7. A command whose earlier runs ended within 5 s starts at once, up to the upper bound.
8. One child cgroup per job; a job's `cpu.weight` falls after it used 10 s of CPU (start), so
   the kernel favours jobs that have run little.
9. Bounds: at least 4 running jobs (start); at most 48, and no more than the predicted peak
   memory allows (start).
10. A job holds no slot of its own beyond the implicit one. The jobserver FIFO keeps bounding
    the compiler processes inside jobs; the daemon takes tokens out or returns them as the
    budget moves.

Not chosen: a larger fixed pool (jobs hit their time limits at 50); a load-average limit (damped,
counts I/O waiters, reworked twice in make); kernel weights alone (no bound on runnable tasks or
memory); CPU pressure as the only target (no sourced value, and it saturates with many tasks).

To measure before building: the compile cache server's CPU per minute beside the jobs' CPU;
jobs per minute and slowdown at budgets of 13, 15 and 17; the host's utilisation stored with
every job record, because history taken under contention understates a job's share.

## Requests for more than CPU

The operator's words: a client could request "cpu shares, memory, disk, io bandwidth", and
"depending on what's available we get scheduled earlier or later".

What the host confines today (read from the daemon's cgroup, 2026-11-22):

- CPU: one quota for all jobs (`cpu.max`, 17 of 20 CPUs with a headroom of 15 %) and one weight
  (`cpu.weight` 20). Nothing per job.
- Memory: no limit (`memory.max` is `max`). Admission waits while the host has under 4 GB free
  (`--min-free-memory`). `--memory-max` puts each job in a scope with a `MemoryMax`; it is off
  and was never run beyond starting jobs.
- Disk: admission waits under `--min-free-disk`; eviction frees old target directories.
- I/O: nothing. The `io` controller is not delegated to the user manager (the daemon's cgroup
  has `cpu memory pids`), so neither `io.weight` nor `io.max` can be set without a root drop-in.
- The compile cache server runs in a unit of its own, outside all of this.

Design:

11. A request names a CPU share and may name memory, disk and I/O. What it leaves out is
    predicted from the job log: peak RSS of earlier runs for memory, the growth of the target
    directory for disk.
12. A job starts when every named resource fits: its CPU share within the budget, its memory
    within the memory free now minus the predicted peaks of the jobs that started in the last
    30 s, its disk above the free-disk floor. A job that fits starts ahead of an older one that
    does not, and waiting raises the older job's turn so it is not passed for ever.
13. Each job runs in a cgroup of its own with `cpu.weight` from its share and `memory.max` from
    its request with a margin, so a job that exceeds its memory is killed alone.
14. I/O is admitted by the host's I/O pressure as a brake until the `io` controller is
    delegated; then a job's cgroup gets `io.weight`. Measured I/O pressure was 5 to 9 % on the
    day of this story: not the bottleneck.

## The token FIFO loses slots when jobs are killed

Measured the same day, after 11 jobs were killed for running over 10 minutes: in three counts
the pool's limit was 50, 0 slots were free, 14 to 17 were granted to running jobs, and the 6 to
8 processes that had the FIFO open (all `cargo`, one build script) had 11 child processes
between them. At least 19 slots were held by no live process. The daemon writes lost slots back
only while no other process has the FIFO open, which does not happen on a busy pool
(`story:lost-tokens-return-on-a-busy-pool`).

A killed `cargo` takes the tokens it drew with it. With admission by CPU share the shared FIFO
is not needed to bound jobs; the open question is whether it stays for the compiler processes
inside a job, or each job gets a job count of its own (`-j` from its share) and no token can be
lost.
