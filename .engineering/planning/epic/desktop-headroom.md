---
format: aep.planning-md/3
id: epic:desktop-headroom
kind: epic
status: draft
title: A full pool leaves the desktop usable
summary: CPU weight for builds, test phase inside the pool, disk guard, memory-max verified, memory-aware admission.
relations:
- serves: vision:O2
- informed_by: specification:delivered-baseline
revision: 1
---
# Epic: a full pool leaves the desktop usable

## Outcome

When every slot is taken, the workstation stays usable and no build takes the machine down with
it: interactive applications (the browser) win CPU over builds, the test phase of a build stays
inside the pool, no build starts on a filesystem that is nearly full, and several link-heavy jobs
at once do not end in the kernel OOM killer.

## Why

O2: the pool is the one place that decides who builds when (`AGENTS.md` § Serves). Today it decides
only how many compile slots run. It does not decide CPU priority against the desktop, test-thread
counts beyond a hint, disk, or memory:

- jobs run at the default CPU weight of whatever started the daemon (`contrib/slot.service` sets
  no `CPUWeight`);
- test binaries get `RUST_TEST_THREADS` / `NEXTEST_TEST_THREADS` = free tokens at job start but
  hold none of them (`crates/slot/src/toolchain.rs`, `Cargo::env`);
- nothing checks free disk space before a build (`docs/design.md` § 1, "disk is the second
  constraint"; the organization rule is 25 GB free);
- `--memory-max` exists, is off by default and is untested (`docs/design.md` § 9, § 10).

## Stories

1. `story:builds-run-at-low-cpu-weight` — the daemon's cgroup carries `CPUWeight=20`; documented
   that a daemon outside systemd has no such protection.
2. `story:memory-max-scope-carries-cpu-weight` — the per-job `--memory-max` scope carries the same
   weight.
3. `story:test-execution-bounded-by-pool` — test threads and doc-tests stay inside the pool.
4. `story:disk-guard` — refuse to start a build below a configured free space.
5. `story:memory-max-is-tested` — `--memory-max` kills only the job that exceeds it.
6. `story:admission-waits-for-memory` — admission holds a job while memory is short.

## Not in this epic

Disk-budget eviction of target directories and reflink seeding (`docs/design.md` § 10): they free
space, this epic only refuses to fill it. CPU weight on a remote build server.

## Acceptance

Done when each story's acceptance holds, and one combined run on the workstation with the pool
full of CPU-bound jobs shows: a busy loop outside the pool on the same cores gets at least 75 % of
one core, no test or doc-test phase pushes runnable build threads above the pool size, and the
daemon is still serving `/api/state` afterwards.
