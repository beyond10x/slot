---
format: aep.planning-md/3
id: task:ux-d01-change-set
kind: task
status: draft
title: 'D-01 change set: underlined links, headroom and plugin on the landing page, local search, no stale version, 24 px carousel dots'
relations:
- decomposes: story:ux-loop-docs-site
revision: 2
---
# Task: ux-d01-change-set

Change set of docs round D-01 (`website/ux/rounds/D-01/triage.md`): DXF-001 to DXF-005.

## Acceptance

- At 1366x768 and 1280x720 in both themes, axe-core reports no link-in-text-block and no
  target-size violation on any page (before: 32 and 4 gating runs).
- The navbar has a search box on every page (before: missing on 44 of 44 gating runs).
- The reader does the headroom and plugin tasks from the landing page in 1 action and no steer at
  1366x768 (before: 2 actions, 1 steer each).
- `website/ux/copy.test.ts` passes, and fails on main's `product.json`.
