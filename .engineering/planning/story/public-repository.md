---
format: aep.planning-md/3
id: story:public-repository
kind: story
status: draft
title: beyond10x/slot exists on GitHub as a public repository
relations:
- decomposes: epic:publish-first-release
- serves: vision:O6
revision: 3
---
# Story: public-repository

## Outcome

`beyond10x/slot` exists on GitHub as a public repository with default branch `main`. Pushing the
tree is not part of this story: the first push is made by `b10x-bot[bot]` once the repository is
enrolled (`story:gates-enrolment`, `AGENTS.md` § Delivery).

## Context

- Repository creation in the organization goes through the steward session b10x-atlas; the
  request is queued (2026-11-12). That wait is recorded as `dependency-blocker:github-repository-request`.

## Acceptance

1. `gh api repos/beyond10x/slot --jq '[.visibility, .default_branch] | join(" ")'` prints
   `public main`.

## Scope

- No change to this tree. The GitHub repository only.
