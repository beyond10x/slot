---
format: aep.planning-md/3
id: story:builds-run-at-low-cpu-weight
kind: story
status: draft
title: Builds run in a cgroup with CPUWeight=20
relations:
- decomposes: epic:desktop-headroom
- serves: vision:O2
- depends_on: story:permanent-service-unit
scope:
- confidence: inferred
  path: contrib/slot.service
- confidence: inferred
  path: website/docs/getting-started.md
revision: 4
---
# Story: builds-run-at-low-cpu-weight

## Outcome

The daemon's own cgroup — the installed `slot.service` unit — carries `CPUWeight=20`, so every job
that stays in it (every job not wrapped by `--memory-max`) loses contested CPU to an interactive
application at the default weight (100) about 5 to 1 when the pool is full. The documentation
states that a daemon started outside systemd (from a shell, `slot serve &`) has no such
protection, and how to get it.

## Context

- `contrib/slot.service` has no `CPUWeight=`; `task service` installs it as is (`Taskfile.yml`).
- Jobs are children of the daemon and stay in its cgroup unless `--memory-max` wraps them in a
  `systemd-run --user --scope` (`crates/slot/src/server/mod.rs`); those jobs are
  `story:memory-max-scope-carries-cpu-weight`, which reuses the value set here.
- Depends on `story:permanent-service-unit` (the weight is a property of the installed unit; both
  edit `contrib/slot.service` and `website/docs/getting-started.md`).

## Acceptance

1. `systemctl --user show slot.service -p CPUWeight` prints `CPUWeight=20` after `task service`.
2. With the pool of N tokens filled by N CPU-bound jobs (`slot sh -c 'while :; do :; done'`) all
   pinned with `taskset` to the same cores as a busy loop started from the desktop session, the
   busy loop gets at least 75 % of one core over 30 s, measured from `/proc/<pid>/stat` utime+stime.
   Without the weight (a daemon started from a shell) the same measurement is recorded for contrast.
3. `website/docs/getting-started.md` contains the sentence that a daemon started outside systemd
   gets no CPU weight protection, and names `task service` as the way to get it.

## Scope

- `contrib/slot.service` (`CPUWeight=20`)
- `website/docs/getting-started.md`
