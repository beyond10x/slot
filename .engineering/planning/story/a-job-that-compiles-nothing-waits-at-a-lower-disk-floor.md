---
format: aep.planning-md/3
id: story:a-job-that-compiles-nothing-waits-at-a-lower-disk-floor
kind: story
status: draft
title: A job that compiles nothing waits at a lower disk floor than a build
relations:
- serves: vision:O2
revision: 1
---
# A job that compiles nothing waits at a lower disk floor than a build

## Why

The disk floor is one value for every job: `--min-free-disk` (40 GB on the workstation), checked
in `short_of()` (`crates/slot/src/server/mod.rs`) on the filesystems of the job's directory and
target. On 2026-11-25 at 02:10 CET twelve jobs queued on the workstation behind it with 37.5 GB
free, among them jobs that write next to nothing: a test run of a script repository, a profiler, a
documentation check. None of them could go to the build node (each had a local reason), so the
floor held them while the node had free slots.

A job's toolchain does not tell a build from a non-build: toolchain `other` covers `task check`,
whose cargo calls run inside the job without a check of their own.

## Acceptance

- A second floor, `--min-free-disk-light` (default 10 GB), applies to a job whose last 5 runs of
  the same repository and argv in the job log used 0 s of compile-server CPU
  (`compile_server_cpu_s`) and had no target directory. Every other job, a first run included,
  waits at `--min-free-disk`.
- No job starts below the light floor, whatever its history: it is the hard floor.
- `slot status` names the floor a waiting job is held by: `builds need 40.0 GB` or `this job
  needs 10.0 GB (no compile in its last 5 runs)`.
- Red first: with 20 GB free, a job with 5 runs and no compile starts, a first run of the same
  command waits, and a job with one compile in its last 5 runs waits.
- Risk named in the design section: a script that starts compiling after 5 runs without writes
  fills the disk down to the hard floor before its record shows it.

## Not decided

The operator picks whether this is built; until then the single floor stays.
