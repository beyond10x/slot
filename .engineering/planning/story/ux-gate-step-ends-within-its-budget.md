---
format: aep.planning-md/3
id: story:ux-gate-step-ends-within-its-budget
kind: story
status: draft
title: The UX gate step of the web UI ends within 120 seconds and fails over it
relations:
- serves: vision:O6
revision: 1
---
## Outcome

`task ui-ux` ends within 120 s of wall time, on the forge's runner and on a workstation that
runs other builds, and fails by itself when it takes longer.

## Context

- The workspace budget: the test step of a gate ends within 120 s, no single test runs longer
  than 60 s, and the step itself fails when it runs over.
- Measured on 2026-11-21 and 2026-11-22 on a 20-core workstation shared with other builds, run
  time without queue wait, from the job log: 148 s (59 tests), 124 s (59 tests), 107 s (64 tests,
  Playwright 1.7 min). Slowest tests of the 107 s run: layout crowded at 1366x768 8.6 s,
  pool-share 8.4 s, busy-failed 8.2 s. In a round run of 139 tests beside other builds, layout
  busy-failed at 1366x768 took 40.8 s.
- Since round R-11 Playwright stops a test of `ui/` at 60 s and prints the ten slowest tests
  (`ui/playwright.config.ts`, `ui/ux/slowest.ts`). Nothing stops the step at 120 s.
- `website/playwright.config.ts` still allows 120 s per test, and `ui/ux/concurrent.e2e.ts`
  240 s.
- Where the time goes is not diagnosed.

## Acceptance

1. `task ui-ux` exits non-zero when its wall time passes 120 s, and prints the time it took.
2. Three runs in a row on the forge's runner end under 100 s.
3. `website/playwright.config.ts` stops a test at 60 s, and `task site-ux` has the same 120 s
   check.
4. The step still prints its ten slowest tests.

## Scope

- `Taskfile.yml`, `ui/playwright.config.ts`, `website/playwright.config.ts`
- `ui/ux/gates.spec.ts`, `ui/ux/concurrent.e2e.ts`
