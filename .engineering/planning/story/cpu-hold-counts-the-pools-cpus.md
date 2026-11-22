---
format: aep.planning-md/3
id: story:cpu-hold-counts-the-pools-cpus
kind: story
status: active
title: The CPU limit for starts counts the CPU of the pool and the compile cache, not work outside them
relations:
- serves: vision:O2
scope:
- confidence: cited
  path: AGENTS.md
- confidence: inferred
  path: CHANGELOG.md
- confidence: cited
  path: crates/slot/src/census.rs
- confidence: cited
  path: crates/slot/src/cli.rs
- confidence: cited
  path: crates/slot/src/cpu_limit.rs
- confidence: inferred
  path: crates/slot/src/server/machine.rs
- confidence: cited
  path: crates/slot/src/server/mod.rs
- confidence: cited
  path: crates/slot/src/server/tests.rs
- confidence: cited
  path: crates/slot/src/status.rs
- confidence: cited
  path: docs/design.md
- confidence: inferred
  path: plugin/skills/building/SKILL.md
- confidence: inferred
  path: ui/src/lib/queue.test.ts
- confidence: inferred
  path: ui/src/lib/queue.ts
- confidence: inferred
  path: website/data/status.json
- confidence: cited
  path: website/docs/how-it-works.md
- confidence: cited
  path: website/docs/reference/cli.md
revision: 19
transitions:
- {from: "draft", to: "proposed", at: "2026-11-22T18:26:43Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-22T18:26:43Z", actor: "human:timo", revision: 3}
---
## Outcome

The CPU limit for starts holds the pool's jobs on the CPU that the pool's own work uses, not on
work that runs outside it.

## Context

- `slot cpu-limit` (0.24.0, `crates/slot/src/cpu_limit.rs`) compares the CPU time of every process
  on the host with the mark.
- Measured 2026-11-22 with the mark 17 and the floor 6: one hour with 6.7 to 10.0 CPUs busy passed
  with a queue wait of 0 s at the median (706 jobs). Then 7.7 to 10.8 of 16 to 20 busy CPUs were
  processes outside the pool and outside the compile cache server; for 11 minutes 78 jobs waited
  204 s at the median, the CPU pressure stayed at 58 to 77 %, and the limit was switched off at
  14:30:11 by its watch.
- The pool's work runs in two places: the daemon's cgroup (`slot.service`) and the compile cache
  server's (`sccache.service`); the census already classifies both (`classes` with `@slot`, unit
  `sccache`).
- `AGENTS.md` open item (aa).

## Acceptance

1. Test: with 10 CPUs busy outside the pool and the cache server, 5 inside, the mark 17 and the
   floor 6, a queued job starts; with 12 inside the pool and 6 in the cache server it is held.
2. The held job's `wait_reason` names the two parts (`pool 12.0 + compile cache 6.0 of 20 CPUs,
   mark 17.0`), and `slot cpu-limit` prints the parts and the CPUs outside both.
3. `GET /api/cpu-limit` answers the parts as fields (`pool`, `compile_cache`, `outside`), each with
   `#[serde(default)]`.
4. `docs/design.md` § 24, the CLI reference and `website/docs` say what the figure counts.
   `AGENTS.md` open item (aa) is removed.
