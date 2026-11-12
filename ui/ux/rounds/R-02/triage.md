# R-02 triage

## Caveats

- crowded was not measured (measure.md); its R-01 findings (UXF-007, UXF-008, UXF-009) are carried
  from R-01.
- No person, no review agents, one theme; as in R-01.

## Did the last change sets move their metrics?

| change | target metric | R-01 | R-02 |
|---|---|---|---|
| UXF-002 slider back on refusal | false_complete, video-call-old-daemon | 1 | 0 |
| UXF-005 badge colour | axe color-contrast, every scenario | 1 per state | 0 |
| UXF-004 command column absorbs width | page sideways scroll at 1280x720 | 66 px in 6 of 6 | 0 in 5 of 5 measured |
| UXF-006 session title in hover text | clipped without full value | 6 scenarios | 0 at gating sizes |
| UXF-001 job number is a button | busy-failed by keyboard | not reachable | done, 23 tabs |

## Findings and priority

| finding | severity | frequency | confidence | cost | priority |
|---|---|---|---|---|---|
| UXF-003 headroom takes 9 actions (decision R-01: yes) | S2 (3) | 2/6 | 1 | M (2) | 0.5 |
| UXF-010 layout shift on load | S3 (1) | 6/6 | 1 | M (2) | 0.5 |
| UXF-008 output text not focusable | S2 (3) | 1/6 | 1 | S (1) | 0.5 |
| UXF-009 live buttons below 24 px | S3 (1) | 1/6 | 1 | S (1) | 0.17 |
| UXF-007 header outside the viewport (crowded, 1280) | S2 (3) | 1/6 | 1 | M (2) | 0.25 |

UXF-007 is not in the set: its R-01 evidence shows the page scrolling sideways in the same frames,
which UXF-004 removed. R-03 checks it.

## Change set R-02 (4 changes)

1. UXF-003: buttons 0 %, 20 %, 40 % beside the slider (decision R-01, default yes).
2. UXF-010: the panels render once the snapshot and the history have loaded; until then one line
   says so.
3. UXF-008: the output text is a focusable `log` region named "output of job N".
4. UXF-009: the live and expand buttons are at least 24 px high.
