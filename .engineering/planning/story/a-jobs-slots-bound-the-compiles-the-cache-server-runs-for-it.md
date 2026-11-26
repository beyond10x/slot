---
format: aep.planning-md/3
id: story:a-jobs-slots-bound-the-compiles-the-cache-server-runs-for-it
kind: story
status: draft
title: A job's slots bound the compiles the cache server runs for it
relations:
- serves: vision:O2
- decomposes: epic:desktop-headroom
- informed_by: story:compile-servers-count-as-pool-load
revision: 1
---
# Story: a-jobs-slots-bound-the-compiles-the-cache-server-runs-for-it

## Outcome

A job holding N slots uses about N CPUs, counting the compiles the compile cache server runs for
it. When its compile-server CPU runs above its share, admission counts the excess as the job's
load, and `slot status` shows the job's CPUs next to its slots.

## Evidence (2026-11-24, 08:28–08:31 CET)

- One local job of 1 slot (`cargo nextest run` in a mutation-testing copy, a cold build of
  tokio, rustls and their dependencies) ran 195 s with `cpu_s` 802.0, of which
  `compile_server_cpu_s` 801.5: 4.1 CPUs on average for 1 slot.
- At 08:31 CET 12 rustc processes ran under the compile cache server for that one job.
- The job's own processes used 0.5 CPU-seconds; its slot bounded nothing it did.

## Recommendation

- Measure first: record, per job, the peak number of concurrent compiles the server ran for it
  (the job log already has `compile_server_cpu_s`).
- Then bound: the compile server takes a jobserver token from the pool for each compile it runs
  beyond the requesting job's held slots, or the job's compile requests queue in the server at its
  slot count.
- `slot status` prints `<cpus> CPUs` per job, including its compile-server CPU.

## Acceptance

- A 1-slot job doing a cold build of a 200-crate workspace through the compile cache server never
  has more than 2 of its compiles running at once, and its average CPU over the run is at or under
  2.
- `slot status` shows that job's CPUs including the server's work for it.
