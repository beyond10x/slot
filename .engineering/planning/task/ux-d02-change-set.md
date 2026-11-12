---
format: aep.planning-md/3
id: task:ux-d02-change-set
kind: task
status: draft
title: 'D-02 change set: search box clear of the menu button on narrow windows, readable search hint'
relations:
- decomposes: story:ux-loop-docs-site
revision: 2
---
# Task: ux-d02-change-set

Change set of docs round D-02 (`website/ux/rounds/D-02/triage.md`): DXF-006 and DXF-007, and the
probe naming the search plugin's input.

## Acceptance

- `website/ux/narrow.spec.ts` passes at 390x844 in both themes: navbar, menu button and search box
  in view and not covered; it fails without the fix.
- axe-core reports no color-contrast violation in the navbar (before: 24 gating runs).
- Every page at all 7 sizes in both themes: 0 problems; every reader task done by pointer and
  keyboard.
