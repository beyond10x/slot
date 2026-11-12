---
format: aep.planning-md/3
id: task:ux-r03-change-set
kind: task
status: draft
title: 'R-03 change set: job tables under the output pane, expand button name, scenario data'
relations:
- decomposes: story:ux-loop-web-ui
revision: 2
---
# Task: ux-r03-change-set

Change set of round R-03 (`ui/ux/rounds/R-03/triage.md`): UXF-012 with UXF-011, UXF-013, and the
scenario data.

## Acceptance

- crowded at 1366x768 and 1280x720: no "runs past its panel" (R-03: Running table 175 px, Finished
  table 8 px).
- 960x1080: no sideways page scroll (R-03: 59 to 62 px in 5 of 6 scenarios).
- `ux/hero.ts` finds the expand button by the name "show units of job N".
