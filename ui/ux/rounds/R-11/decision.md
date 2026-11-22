# R-11 decision: do the Queue, Running and Finished tables move above the host panel?

## Problem

UXF-020: at 1366x768 the "Queue" heading starts at page y 1001 (idle) to 1405 (crowded) in 11 of
11 scenarios, and below the first screen at every measured size from 960x1080 to 1920x1080. Three
of the seven scenario tasks are about a job (busy-failed, queue-position, queue-levels); each
starts with a scroll: `screen_only.looks` is 3 in 15 of 15 pointer runs of those tasks. The first
screen holds the host chart, two small charts, the Sessions table and the top of the timeline.

## Options

- Yes: the Queue, Running and Finished tables come directly under the header; the host panel, the
  Sessions table and the timeline follow. Shot: [shots/decision-yes.png](shots/decision-yes.png).
  Cost: the host chart and the timeline leave the first screen when jobs are listed; the baseline
  of every scenario moves (reading load up, looks down).
- No: the order stays. Shot: [shots/decision-no.png](shots/decision-no.png). The header's failed
  count (UXF-019) and "queue n" stay the only trace of a job on the first screen.

A third form, folding the host panel to one row, is not offered: it needs a new control and a
stored state, and UXF-023 (the header's height) would have to be settled first.

## Target metric

`screen_only.looks` of busy-failed, queue-position and queue-levels at 1366x768: now 3, expected
1 or 2. `screen_only.actions` of queue-position: expected not higher than now.

## Default

Yes, had it stayed unanswered until round R-12: the tasks the scenarios name are about jobs, and the host panel
answers no scenario task but machine-headroom, whose answer is in the header line.

## Answer

Yes, 2026-11-22. Built in this round as part of the R-11 change set.

After, at 1366x768: `screen_only.looks` 1 for busy-failed and 2 for queue-position and
queue-levels (was 3); `screen_only.actions` of queue-position 0 (was 1). The cost, as named under
"Options": the reading load of the first screen went up in 6 scenarios and the layout shift in 2
(UXF-032). Those entries of `ux/baseline.json` name this decision as `R-11-job-tables-first`.
