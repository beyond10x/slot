---
format: aep.planning-md/3
id: story:a-job-can-run-in-the-callers-cgroup
kind: story
status: draft
title: A job can run in the caller's cgroup and still hold its slots
revision: 1
---
## Outcome

A job whose test needs the caller's cgroup (a delegated systemd scope, a cgroup limit the caller
set up) can run there and still hold its slots in the pool.

## Evidence

2026-11-22, a finding from another repository on this host: one test binary run directly passed
10 of 10; the same binary started through the `cargo` shim exited 101, because the daemon spawns
every job as its own child, so the job runs in `slot.service`'s cgroup, outside the delegated
scope the test expects. The hosted CI lane, without slot, is green.

Today the only ways to stay in the caller's cgroup are `slot --local` and `SLOT_SHIM_OFF=1`, and
both run outside the pool.

## Acceptance

1. A documented option runs the job's process in the caller's cgroup while the daemon still
   admits it, counts its slots and records it in the job log.
2. A test shows a job started with that option reads the caller's `/proc/self/cgroup`, and one
   without it reads the daemon's.
3. The job's run-time limit and cancel still end every process of the job.

## Not in scope

Moving every job into its caller's cgroup by default.
