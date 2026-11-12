---
format: aep.planning-md/3
id: dependency-blocker:github-repository-request
kind: dependency-blocker
status: open
title: The request to create beyond10x/slot is queued with the steward session
relations:
- blocks: story:public-repository
revision: 1
---
# Dependency blocker: github-repository-request

`story:public-repository` cannot start: repositories in the `beyond10x` organization are created
by the steward session b10x-atlas, and the request to create `beyond10x/slot` is queued there
(2026-11-12), not yet answered.

## Clears when

`gh api repos/beyond10x/slot` answers with HTTP 200 (the repository exists). Then this blocker is
moved to `cleared`.
