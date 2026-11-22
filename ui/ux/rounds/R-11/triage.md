# R-11 triage

## Caveats

- No person, one theme, one run per scenario and size: 125 hermetic tests, one result file each.
  Two review agents read the round (`review-heuristics.md`, `review-report.md`); it is the first
  round with both.
- The numbers under "Before" are the first measurement of the round, at 1366x768. `report.md` is
  measured after the change set.
- `layout_shift.per_minute` was the layout shift of one page load, not a rate (the divisor was
  clamped to 1 minute). The key is now `layout_shift.total`, same value.
- Reading load and layout shift are in the tables at 1366x768 only. The largest values of the
  round are at sizes the report does not print: layout shift 0.122 (crowded at 960x1080), 870
  words in view (`review-report.md` § 1.4).
- pool-share has no task and its first screen equals busy-failed's: the usage table is measured
  by nothing.
- "by keyboard: done" is a keyboard result for busy-failed and the two video-call scenarios only;
  the other keyboard runs press no key (`review-report.md` T6).
- The round ratchet failed with 10 lines before the change set, and no task ran it. One line is a
  worse value: upgrade-pending layout shift 0.088 against a baseline of 0.005. R-09 read 0.088,
  R-10 0.005, R-11 0.088, with the same pixels in the `opened` frame of all three rounds. The
  shift happens in the first 0.6 s of the page load, when the header fills and wraps
  (`review-heuristics.md`). Decision `R-11-upgrade-pending-shift`: the baseline of 0.005 was one
  low sample; the entry is set to 0.088, read in three measurements of this round (before the
  change set, after it, and after `main` was merged in), and the header's growth on load stays
  open as UXF-023.
- Nine lines were values of machine-headroom, pool-share and queue-levels that had no baseline:
  the merge-mode `--update` does not write the host-dependent keys.

## Did the last change sets move their metrics?

| change | target metric | before | after |
|---|---|---|---|
| R-10: opening the bottom sheet scrolls its opener into sight (UXF-018) | busy-failed at 960x1080, opener out of sight after opening | 2 of 2 runs | 0 of 2 runs |

## Findings and priority

priority = severity weight × frequency × confidence ÷ cost. Frequency is scenarios affected of 11.
Confidence is 2 where a metric of the round and the heuristic review agree.

| finding | severity | frequency | confidence | cost | priority |
|---|---|---|---|---|---|
| UXF-020 job tables start below the first screen | S2 (3) | 11/11 | 2 (looks 3 in 15 of 15 pointer runs) | M (2) | 3.00 |
| UXF-019 a failed build is not named on the first screen | S2 (3) | 3/11 | 2 (busy-failed: 3 looks, 2 actions) | S (1) | 1.64 |
| UXF-022 an opened output is 15 tabs from its job | S2 (3) | 3/11 | 2 (busy-failed: 26 tabs) | S (1) | 1.64 |
| UXF-021 unit rows open and close by themselves | S2 (3) | 4/11 | 1 | S (1) | 1.09 |
| UXF-029 one state has several names | S3 (1) | 11/11 | 1 | S (1) | 1.00 |
| UXF-028 the header speaks in the daemon's terms | S3 (1) | 11/11 | 1 | S (1) | 1.00 |
| UXF-023 the sticky header takes 15 to 27 % of the screen | S3 (1) | 11/11 | 2 (machine-headroom: 195 of 768 px) | M (2) | 1.00 |
| UXF-027 a session is 8 hex digits in the job tables | S3 (1) | 10/11 | 1 | S (1) | 0.91 |
| UXF-030 running bars blink, also under reduced motion | S3 (1) | 10/11 | 1 | S (1) | 0.91 |
| UXF-025 a queued job's output is an empty pane marked live | S3 (1) | 8/11 | 1 | S (1) | 0.73 |
| UXF-026 26 tabs to job 42 | S3 (1) | 3/11 | 2 (busy-failed: 26 tabs) | S (1) | 0.55 |
| UXF-024 reasons and full values only in hover titles | S3 (1) | 11/11 | 1 | M (2) | 0.50 |
| UXF-031 no text but the title is 14 px or larger | S3 (1) | 11/11 | 1 | M (2) | 0.50 |

No S1. UXF-029, UXF-028 and UXF-023 tie at 1.00; UXF-029 takes the fifth place because its
evidence is on the anchor card busy-failed.

## Change set R-11 (5 changes)

1. UXF-020: the Queue, Running and Finished tables come directly under the header; the host
   panel, the Sessions table and the timeline follow. A layout change: `decision.md`, answered
   yes during the round.
2. UXF-019: the header counts the jobs that failed in the last 60 minutes; the count opens the
   output of the newest one.
3. UXF-022: opening an output by keyboard moves focus into the pane; Escape closes the pane and
   returns focus to the control that opened it.
4. UXF-021: a unit row opens and closes only by the user's toggle.
5. UXF-029, three parts: the Finished table states the result in words ("failed (101)", "ok",
   "cancelled"); a build outside the pool is "not on slot" everywhere; the Queue heading does not
   say "waiting for slots" for a job that waits for disk space or memory.

Before (1366x768): busy-failed by pointer 2 actions and 3 looks, by keyboard 26 tabs; after Enter
on "output of job 42" the pane's "close" is 17 Tab presses away and Escape leaves the pane open;
crowded opens unit rows of jobs 100, 104, 108, then 101, 108, 112, then 101, 104, 108 in three
frames 1 s apart.

Target metrics: busy-failed `screen_only.actions` 2 to 1 and `screen_only.looks` 3 to 1 (UXF-019);
"close" at most 2 Tab presses from the opener's Enter (UXF-022); 0 Running rows moved without
input over three ticks in crowded (UXF-021).

After (1366x768, measured with `main` merged in): busy-failed by pointer 1 action and 1 look, by
keyboard 5 tabs; queue-position and queue-levels 0 actions and 2 looks (were 1 and 3). The three
target metrics are asserted in `ux/findings.spec.ts`.

Cost of UXF-020, taken into the baseline under decision `R-11-job-tables-first`: the reading load
of the first screen went up in 6 scenarios (busy-failed 202 to 386, call-mode 205 to 353,
queue-position 202 to 365, video-call and video-call-old-daemon 202 to 372, upgrade-pending 214
to 333) and the layout shift in 2 (crowded 0.044 to 0.089, busy-failed 0.035 to 0.049), filed as
UXF-032. Reading load of crowded went down, 548 to 472.

## Tools

Changes to the loop itself, from `review-report.md` § 3. They are not counted in the 5.

- T1: a test that crashed wrote no file and the round read that as a pass. `round.ts` lists every
  expected file that is missing, counts it and exits 1.
- T2: `layout_shift.per_minute` is renamed `layout_shift.total`.
- T3: `task ux-round` runs the round ratchet, and `report.md` shows its lines.
- A Playwright test is killed at 60 s (was 120 s), and the run prints its ten slowest tests.
- Left open: T4 to T12.
