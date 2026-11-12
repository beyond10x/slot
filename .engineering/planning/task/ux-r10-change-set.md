---
format: aep.planning-md/3
id: task:ux-r10-change-set
kind: task
status: draft
title: 'R-10 change set: the bottom-sheet output pane keeps the job that opened it in sight'
relations:
- decomposes: story:ux-loop-web-ui
revision: 2
---
Change set of round R-10 (`ui/ux/rounds/R-10/triage.md`): UXF-018 (the bottom-sheet output pane
keeps the control that opened it in sight), after a trial that runs the screen-only bot at every
size in a round.

## Acceptance

- Scenario busy-failed at 960x1080: after opening the output of job 42 its control is between the
  header and the sheet, by pointer and by keyboard (before: out of sight in 2 of 2 runs).
- The gating sizes keep 0 gate problems and every screen-only task done.
