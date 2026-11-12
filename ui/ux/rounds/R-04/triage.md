# R-04 triage

## Caveats

- No person, no review agents, one theme; as in R-01.

## Did the last change sets move their metrics?

| change | target metric | R-03 | R-04 |
|---|---|---|---|
| UXF-012 detail columns drop below 1150 px | runs past its panel, crowded at 1366x768 | 175 px and 8 px | 0 |
| UXF-012 | runs past its panel, crowded at 1280x720 | not measured (probe added in R-03) | 21 px |
| UXF-011 (same change) | sideways page scroll at 960x1080 | 59 to 62 px in 5 of 6 | 0 |
| UXF-013 expand button name | hero script finds "show units of job N" | not found | found |

## Findings and priority

| finding | severity | frequency | confidence | cost | priority |
|---|---|---|---|---|---|
| UXF-014 Running table 21 px under the pane, crowded at 1280x720 | S2 (3) | 1/6 | 1 | S (1) | 0.5 |
| UXF-015 job tables 119 to 219 px under the pane at 960x1080 (measured-only) | S3 (1) | 1/6 | 1 | M (2) | 0.08 |

## Change set R-04 (1 change)

1. UXF-014: the repository cell cuts at 14em with an ellipsis; the full name is its title.

UXF-015 stays open: 960x1080 with the output pane open leaves the tables 595 px.
