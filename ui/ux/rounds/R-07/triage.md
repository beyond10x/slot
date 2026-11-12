# R-07 triage

## Caveats

- The finding of this round came from the gate job on GitHub Actions, not from a round on the
  workstation: CI's fonts are wider. The round report (`report.md`) is measured after the change.
- No person, no review agents, one theme; as in R-01.

## Did the last change sets move their metrics?

No change set in R-06.

## Findings and priority

| finding | severity | frequency | confidence | cost | priority |
|---|---|---|---|---|---|
| UXF-016 a table 12 px under the pane at 1280x720 on CI (guessed: Running) | S2 (3) | 1/7 | 1 | S (1) | 0.43 |
| UXF-015 job tables under the pane at 960x1080 (measured-only) | S3 (1) | 1/7 | 1 | M (2) | 0.07 |

## Change set R-07 (1 change)

1. UXF-016: below 900 px the Running table drops the live-output button column.

R-08 showed the guess wrong: the table was the Sessions table (see R-08 triage).
