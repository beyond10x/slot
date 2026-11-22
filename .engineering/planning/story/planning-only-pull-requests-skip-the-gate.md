---
format: aep.planning-md/3
id: story:planning-only-pull-requests-skip-the-gate
kind: story
status: implemented
title: A pull request of planning or prose files passes its checks without building the code
relations:
- serves: vision:O6
scope:
- confidence: inferred
  path: .github/scripts
- confidence: cited
  path: .github/workflows/gate.yml
- confidence: cited
  path: .github/workflows/pages.yml
- confidence: inferred
  path: AGENTS.md
- confidence: cited
  path: Taskfile.yml
- confidence: inferred
  path: crates/slot-docs/tests
revision: 10
transitions:
- {from: "draft", to: "proposed", at: "2026-11-22T18:26:42Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-22T18:26:42Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-22T20:50:26Z", actor: "human:timo", revision: 10, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome

A pull request that changes only planning or prose files passes its required checks without
building or testing the code.

## Context

- The required checks are `common / Security and privacy`, `gate` and `Build documentation`.
  `gate` runs `task check` for every pull request (`.github/workflows/gate.yml`).
- Measured: a pull request of three files under `.engineering/planning/` and `AGENTS.md` ran
  `task check` for 307 s (the `gate` job 5 min 41 s) and the documentation build for 90 s.
- The workspace rule: the required job runs, finds only such paths, and exits 0; one path list
  per repository, in the workflow, with a test case for both sides of it. A change to the built
  docs site (`website/`) still runs the docs check.

## Acceptance

1. A pull request whose changed files are all under `.engineering/`, or `*.md` outside
   `website/`, passes `gate` without running `task check`; the job's log names the paths it saw.
2. A pull request that changes one file outside that list runs `task check` as today.
3. The path list is one file or one block in the workflow, and a test feeds it one list of paths
   from each side (only planning files; planning files plus one source file) and checks the answer.
4. A pull request that changes only `website/` runs the docs checks and not the code's build and
   tests.
5. The ruleset and the names of the required checks are unchanged.

## Scope

- `.github/workflows/gate.yml`, `.github/workflows/` (the documentation build), `Taskfile.yml`
- a script for the path decision and its test
