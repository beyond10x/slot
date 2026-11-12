# R-09 triage

## Caveats

- No person, no review agents, one theme; as in R-01.
- R-09 adds the scenario `upgrade-pending` (a reload waits for running jobs and holds new ones)
  with a `see` task: the bot passes when the reason is text on the page. It was written before the
  change that shows it. The numbers before the change set are in the table below; `report.md`
  is measured after it.
- The history ticks of `upgrade-pending` first lacked the line, so the badge vanished when history
  loaded and came back with the next tick (layout shift 0.571 at 960x1080). A daemon's history
  ticks carry it; the scenario was corrected.

## Did the last change sets move their metrics?

| change | target metric | before | after |
|---|---|---|---|
| R-08: DejaVu Sans pinned; Sessions table drops two columns below 1150 px | gate problems at 1280x720 on CI | 1 | 0 (R-08 and R-09) |

## Findings and priority

| finding | severity | frequency | confidence | cost | priority |
|---|---|---|---|---|---|
| UXF-017 pending upgrade holds builds, page does not say so | S2 (3) | 1/8 | 3 (4 of 4 bot runs) | M (2) | 0.56 |
| UXF-015 job tables under the pane at 960x1080 (measured-only) | S3 (1) | 1/8 | 3 (3 tables, every round since R-04) | M (2) | 0.19 |

UXF-015 is in this change set by the operator's request.

## Change set R-09 (2 changes)

1. UXF-017: every tick carries `upgrade` (the line `slot status` prints); the store reads it from
   the snapshot and the ticks; the header shows it while it is not empty.
2. UXF-015: below 1100 px the output pane is a sheet along the bottom, 45 % of the window high,
   and the tables take the full width (`decision.md`).

Also in this change: the gate harness serves `ui/dist` on a free port instead of 4317, so two
worktrees can run `task check` at the same time (`ux/port.ts`, `ux/concurrent.e2e.ts`).
