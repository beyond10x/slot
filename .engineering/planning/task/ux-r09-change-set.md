---
format: aep.planning-md/3
id: task:ux-r09-change-set
kind: task
status: draft
title: 'R-09 change set: pending upgrade in the header, output pane as a bottom sheet below 1100 px'
relations:
- decomposes: story:ux-loop-web-ui
revision: 2
---
Change set of round R-09 (`ui/ux/rounds/R-09/triage.md`): UXF-017 (a pending upgrade shown in
the header) and UXF-015 (output pane as a bottom sheet below 1100 px).

## Acceptance

- Scenario upgrade-pending: the screen-only bot reads the upgrade line by pointer and by keyboard
  at 1366x768 and 1280x720 (before: 4 of 4 runs found no such text).
- Scenario crowded at 960x1080: 0 tables run past their panel (before: 3, by 70, 90 and 135 px).
