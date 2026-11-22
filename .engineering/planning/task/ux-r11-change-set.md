---
format: aep.planning-md/3
id: task:ux-r11-change-set
kind: task
status: draft
title: 'R-11 change set: a failed build on the first screen, keyboard focus in the output pane, unit rows that stay put, one name per state'
relations:
- decomposes: story:ux-loop-web-ui
revision: 1
---
Change set of round R-11 (`ui/ux/rounds/R-11/triage.md`), the first round read by two review
agents: 13 findings (UXF-019 to UXF-031), 5 in the set, and three repairs of the loop's own tools.

## Acceptance

- UXF-019: in scenario busy-failed the screen-only user reaches the failed job's output from the
  first screen, by pointer in 1 action (before: 2 actions, 3 looks) and by keyboard; the header
  shows no failed count in scenario idle.
- UXF-022: after Enter on "output of job 42" focus is inside the pane, "close" is at most 2 Tab
  presses away (before: 17), and Escape closes the pane and returns focus to the opener.
- UXF-021: in scenario crowded no unit row opens and no Running row moves over three ticks
  without input (before: the open set changed in each of three frames).
- UXF-029: the Finished table states the result in words; a build outside the pool is "not on
  slot" in the header, the Sessions heading and the chart; the Queue heading does not say "waiting
  for slots" for a job that waits for disk space or memory.
- UXF-020 waits for the answer in `ui/ux/rounds/R-11/decision.md`; it is built in the round after
  the answer or the default.
- Tools: a result file missing from `ui/ux/out` fails the round and is listed in its report; the
  layout-shift key is `layout_shift.total`; `task ux-round` runs the round ratchet and the report
  shows its lines; a Playwright test is killed at 60 s.
- The gating sizes keep 0 gate problems and every screen-only task done; the round ratchet passes
  against the baseline committed with the change set.
