# R-03 triage

## Caveats

- No person, no review agents, one theme; as in R-01.
- First round with every scenario measured after both change sets.
- A trial outside the gates found two more: the docs hero screenshots (1600x1000, output pane open)
  showed the Running table under the pane, and the hero script could not find the expand button by
  name. The probe then gained "runs past its panel" (a child wider than its panel, which a covered
  overflow does not turn into page scroll); at 1366x768 in crowded it reports the Running table 175 px
  and the Finished table 8 px past the left panel.

## Did the last change sets move their metrics?

| change | target metric | before | R-03 |
|---|---|---|---|
| UXF-003 presets 0/20/40 % | screen_only.actions, video-call | 9 | 1 |
| UXF-003 presets 0/20/40 % | keyboard, video-call | 8 key presses after 1 tab | Enter after 4 tabs |
| UXF-010 render once loaded | layout shift total at 1366x768 | 0.137 to 0.547 | 0.034 to 0.040 |
| UXF-010 render once loaded | layout shift total at 1920x1080 | 0.818 to 0.849 | 0.061 to 0.122 |
| UXF-008 focusable output | axe scrollable-region-focusable, crowded | 1 per state | 0 |
| UXF-009 24 px targets | axe target-size, crowded | 1 to 2 per state | 0 |
| UXF-004 (R-01) | UXF-007 header outside the viewport, crowded at 1280 | 3 states | 0 |

busy-failed by keyboard takes 26 tabs (R-02: 23): the three preset buttons sit before the tables in
the tab order.

## Findings and priority

| finding | severity | frequency | confidence | cost | priority |
|---|---|---|---|---|---|
| UXF-012 job tables run under the output pane | S2 (3) | 1/6 | 2 (probe and screenshot) | M (2) | 1.0 |
| UXF-011 sideways scroll at 960x1080 (measured-only) | S3 (1) | 5/6 | 1 | M (2) | 0.42 |
| UXF-013 expand button named "▸" | S3 (1) | 4/6 | 1 | S (1) | 0.67 |

## Change set R-03 (3 changes)

1. UXF-012 and UXF-011, one change: the left panel is a size container; below 1150 px it drops the
   detail columns (sparkline, RSS, session id; average cores and peak RSS in Finished).
2. UXF-013: the expand button is named "show units of job N" / "hide units of job N".
3. Scenario data: running jobs use 1.2 cores on average (was 2.6, more than the 20-core host in
   crowded), and the failed job 42 builds the repository its output names.
