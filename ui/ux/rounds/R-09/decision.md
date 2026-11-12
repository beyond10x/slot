# R-09 decision: below 1100 px the output pane is a bottom sheet

## Problem

UXF-015: at 960x1080 with the output pane open the pane takes at least 320 px, and the Running,
Finished and Sessions tables run 135, 90 and 70 px under it (R-09 report, scenario crowded).

## Options

1. Drop more table columns below 960 px. Each table loses facts, and the next narrow window or
   longer repository name brings the overflow back.
2. Below a width, stack the pane under the tables in the grid. The output is then below every
   table, a page of scrolling away from the job that was clicked.
3. Below 1100 px the pane is fixed along the bottom, 45 % of the window high, and the page gets
   the same padding at its end, so every row can still be scrolled clear of the sheet.

## Decision

Option 3. The tables keep every column at the full width; the output stays in view while the
user scrolls the tables; at 1280 px and wider nothing changes.

## Check

`task ux-round` at 960x1080: 0 problems in scenario crowded; the gating sizes are unchanged.
