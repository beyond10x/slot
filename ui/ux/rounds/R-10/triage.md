# R-10 triage

## Caveats

- No person, no review agents, one theme; as in R-01.
- Trial change: in a round (`UX_ALL=1`) the screen-only bot now runs at every size, not only at
  the two gating sizes, and after opening an output it checks that the control it used is still
  in sight (not under the pane or the header). The ratchet still reads the bot at 1366x768 only.

## Did the last change sets move their metrics?

| change | target metric | before | after |
|---|---|---|---|
| R-09: header shows a pending upgrade (UXF-017) | upgrade-pending screen-only runs failed | 4 of 4 | 0 of 4 |
| R-09: bottom-sheet output pane below 1100 px (UXF-015) | crowded at 960x1080, tables past their panel | 3 (70, 90, 135 px) | 0 |

## Findings and priority

| finding | severity | frequency | confidence | cost | priority |
|---|---|---|---|---|---|
| UXF-018 bottom sheet covers the job row that opened it | S2 (3) | 1/8 | 3 (pointer and keyboard) | S (1) | 1.13 |

## Change set R-10 (1 change)

1. UXF-018: when the pane is a bottom sheet, opening it scrolls the control that opened it
   between the header and the sheet (`revealDelta` in `src/lib/scroll.ts`).

Before: busy-failed at 960x1080, the control of job 42 out of sight after opening in 2 of 2 runs.
`report.md` is measured after the change set.
