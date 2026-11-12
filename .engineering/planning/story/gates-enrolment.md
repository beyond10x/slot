---
format: aep.planning-md/3
id: story:gates-enrolment
kind: story
status: draft
title: slot is enrolled in the gates and its gate runs green on main
relations:
- decomposes: epic:publish-first-release
- serves: vision:O6
- depends_on: story:public-repository
scope:
- confidence: inferred
  path: .github
revision: 3
---
# Story: gates-enrolment

## Outcome

slot is enrolled in the organization's gates, and `b10x-bot[bot]` pushes the first commit through
`b10x-gates bot --repository beyond10x/slot`. On that commit and every later push to `main` and
pull request, the shared source gates (`.github/workflows/shared-gates.yml`, reusable workflow
`beyond10x/gates/.github/workflows/common.yml`) and the repository gate
(`.github/workflows/gate.yml`, `task check`) run, and both are required checks on `main`.

## Context

- `shared-gates.yml` reads the secrets `B10X_GATES_POLICY` / `B10X_GATES_POLICY_GZ`; they exist only
  after enrolment.
- The first commit is the working tree as it stands when enrolment completes. Stories that change
  source and land before that point are in it; the rest land as later commits. No other story waits
  on it, so it imposes no order on them.
- `AGENTS.md` § Never: no private names, home paths, host names or planning ids in source, pages
  or commit messages; `ui/dist/`, `ui/node_modules/`, `website/build/` and `target/` are not
  committed. The first commit is checked against both before the push.
- Depends on `story:public-repository`.

## Acceptance

1. `gh api repos/beyond10x/slot/commits/main --jq .author.login` prints `b10x-bot[bot]`.
2. `gh run list --repo beyond10x/slot --branch main --workflow gate.yml --limit 1 --json conclusion --jq '.[0].conclusion'`
   prints `success`; the same for `shared-gates.yml`.
3. `gh api repos/beyond10x/slot/branches/main/protection --jq '.required_status_checks.contexts'`
   names both workflows' jobs.
4. `git ls-tree -r --name-only origin/main | grep -E '^(ui/dist|ui/node_modules|website/build|target)/'`
   prints nothing.

## Scope

- No source change expected. If enrolment needs a file in this tree, it lands in `.github/`.
