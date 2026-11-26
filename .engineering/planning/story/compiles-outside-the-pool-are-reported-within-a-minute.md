---
format: aep.planning-md/3
id: story:compiles-outside-the-pool-are-reported-within-a-minute
kind: story
status: draft
title: A compile outside the pool is reported within a minute with its session, tree and command
relations:
- serves: vision:O2
- decomposes: epic:desktop-headroom
revision: 1
---
# Story: compiles-outside-the-pool-are-reported-within-a-minute

## Outcome

A compiler process (rustc, cc1, cc1plus, go's compile) on the host that belongs to no pool job is
seen within 60 s. `slot status`, the live view and the job log name it: the owning session (from
the process tree's session id), the working tree, the command at the top of the tree that started
it, the number of compiler processes and their CPU. A process run by the compile cache server
for a pool job is not counted as outside.

## Evidence (2026-11-24, 08:30 CET, a 40-slot host)

- Load average 110 with 26 rustc processes while `slot status` showed 2 local jobs.
- About 20 of them came from one `task test-step` → an xtask binary → `cargo test --workspace
  --no-run` with no `SLOT_JOB` in its environment: the xtask called rustup's cargo by path,
  past the agent `PATH` shim.
- Another came from `cargo xtask render-bundle` on a second toolchain, also with no `SLOT_JOB`.
- Both were found by hand, from `/proc`, after the operator asked why the CPU was at 100 %.

## Recommendation

- The daemon's census (`/proc` sampling, already every interval) classifies each compiler process
  by the nearest ancestor that carries `SLOT_JOB`, or is the compile cache server.
- One without either is "outside the pool". It is grouped by its top non-shell ancestor below the
  session's process, and kept in state as `outside` entries with first-seen time.
- `slot status` prints `outside the pool: <n> compilers, <cpu> CPUs, <session> in <tree>: <command>`
  per group. The web view shows the same row. The job log writes one record per group when it ends.
- Admission counts their CPU as load (it already reads host load; this names it).

## Acceptance

- A test starts `rustc` in a child with no `SLOT_JOB` and the test's own session id: within 60 s
  `slot status` lists it with that session id, tree and command.
- A rustc started by the compile cache server for a pool job is not listed.
- When the process ends, one job-log record names the group, its duration and CPU seconds.
