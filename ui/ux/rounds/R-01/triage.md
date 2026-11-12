# R-01 triage

## Caveats

- First round: no earlier change set, so the "did the last change sets move their metrics?" table is empty.
- No person ran the cards; the screen-only user played every task (§ 7). L4, L5 and L6 did not run:
  slot has no recorded sessions and no keyed replay. The two review agents (L7) did not run.
- One theme: the UI ships a light theme only.
- Frequency is scenarios affected ÷ 6 scenarios.

## Did the last change sets move their metrics?

| change | target metric | before | after |
|---|---|---|---|
| none (first round) | | | |

## Findings and priority

priority = severity weight × frequency × confidence ÷ cost

| finding | severity | frequency | confidence | cost | priority |
|---|---|---|---|---|---|
| UXF-002 refused headroom shown as applied | S1 (9) | 1/6 | 1 | S (1) | 1.5, in by rule 1 |
| UXF-005 badge contrast | S2 (3) | 6/6 | 1 | S (1) | 3.0 |
| UXF-004 page scrolls sideways at 1280 | S2 (3) | 6/6 | 1 | M (2) | 1.5 |
| UXF-006 session title cut without full text | S3 (1) | 5/6 | 1 | S (1) | 0.83 |
| UXF-001 output not reachable by keyboard | S2 (3) | 1/6 | 1 | S (1) | 0.5, anchor card |
| UXF-003 headroom takes 9 actions | S2 (3) | 2/6 | 1 | M (2) | 0.5, anchor card; needs a design decision |
| UXF-007 header outside the viewport (crowded, 1280) | S2 (3) | 1/6 | 1 | M (2) | 0.25 |
| UXF-008 output text not focusable | S2 (3) | 1/6 | 1 | S (1) | 0.5 |
| UXF-009 live buttons below 24 px target | S3 (1) | 1/6 | 1 | S (1) | 0.17 |
| UXF-010 layout shift 0.14–0.85 per load | S3 (1) | 6/6 | 1 | M (2) | 0.5 |

The tie at 0.5 goes to the anchor-card findings, UXF-001 before UXF-003 (cost S before M).

## Change set R-01 (5 changes)

1. UXF-002: a refused change puts the slider back on the daemon's value.
2. UXF-005: badge background #A94300.
3. UXF-004: the command column absorbs the width the other columns leave.
4. UXF-006: the session cell's title holds the full title and the id.
5. UXF-001: the job number is a button "output of job N" in the Queue, Running and Finished tables.

UXF-003 waits for its decision (decision.md) and goes into R-02.
