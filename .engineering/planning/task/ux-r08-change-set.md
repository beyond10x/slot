---
format: aep.planning-md/3
id: task:ux-r08-change-set
kind: task
status: draft
title: 'R-08 change set: one font for the UX gates on every host, Sessions table fits at 1280 px'
relations:
- decomposes: story:ux-loop-web-ui
revision: 2
---
# Task: ux-r08-change-set

Change set of round R-08 (`ui/ux/rounds/R-08/triage.md`): the pinned font in the harness and
UXF-016 (Sessions table).

## Acceptance

- The gate job on GitHub Actions passes crowded at 1280x720 (before: 12 px past the panel).
- The workstation and CI run the gates in the same font.
