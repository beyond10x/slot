# R-08 triage

## Caveats

- From this round the harness renders every page in DejaVu Sans, so the workstation and CI measure
  the same layout. Reading load and layout shift of R-08 are not comparable with R-01 to R-07,
  which used the workstation's Noto Sans; the baseline is rewritten from R-08.
- No person, no review agents, one theme; as in R-01.

## Did the last change sets move their metrics?

| change | target metric | before | after |
|---|---|---|---|
| R-07: Running table drops the live column below 900 px | CI gate, crowded at 1280x720 | 12 px past the panel | 12 px past the panel |

Rule 4: the R-07 change did not move its metric. Kept: below 900 px the job number opens the same
output, so the column was a second way to one place, and it gives the Running table about 60 px.

## Findings and priority

| finding | severity | frequency | confidence | cost | priority |
|---|---|---|---|---|---|
| UXF-016 Sessions table 12 px under the pane at 1280x720 (DejaVu Sans) | S2 (3) | 1/7 | 2 (CI and the workstation with the pinned font) | S (1) | 0.86 |
| UXF-015 job tables under the pane at 960x1080 (measured-only) | S3 (1) | 1/7 | 1 | M (2) | 0.07 |

## Change set R-08 (2 changes)

1. The harness pins DejaVu Sans; the probe names the table that runs past its panel by its heading.
2. UXF-016: below 1150 px the Sessions table drops its memory and sparkline columns.

The report (`report.md`) is measured after the change set: 0 gate problems at both gating sizes.
