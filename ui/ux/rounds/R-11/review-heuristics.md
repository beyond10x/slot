# R-11 heuristic review

4 pass, 8 partial, 4 fail

Basis: the 29 contact-sheet frames at 1366x768, the frames of the other four sizes in
`ux/out/gates/shots`, the probe and screen-only results in `ux/out`, and 18 extra frames
(`shots/review-*.png`) taken from the same hermetic scenarios: below the first screen, the open
pane at 1366, 1280 and 960 px, keyboard focus, hover, a queued job's pane, the headroom presets.
One theme (light). No recordings and no people: the replays stand in, so "evidence" is always a
frame, never a second of a recording. 11 scenarios, 7 with a task.

Severity as used here: S1 the task fails or ends wrong; S2 the task is done with a wrong first
move or clear extra work; S3 friction.

## Verdicts

| # | heuristic | verdict | evidence |
|---|---|---|---|
| 1 | One next thing | partial | `shots/busy-failed@1366x768-build-failed.png` is `-loaded.png` with other numbers: a build failed and nothing names it (UXF-019). Where a state has a next step the page says it: `shots/review-headroom-40-old-daemon.png` ("upgrade it (systemctl --user reload slot)"), `shots/upgrade-pending@1366x768-loaded.png`. |
| 2 | State in words | partial | In words: "upgrade pending: …" (`shots/upgrade-pending@1366x768-loaded.png`), "queue empty", "nothing running" (`shots/review-queue-idle.png`), "ended (exit 101)" (`shots/review-pane-42-keyboard.png`). Not in words: a queued job's pane is blank under "● live" (`shots/review-pane-queued-79.png`, UXF-025); a failed job is a red "101" among zeros (`shots/review-finished-busy-failed.png`, UXF-029); the connection is a green dot with its words in a hover title. |
| 3 | Nothing required hidden | fail | `shots/review-full-busy-failed.png`: Queue starts at page y 1158, Finished at 1614, on a 768 px screen; below the first screen in 11 of 11 scenarios and at all five sizes (UXF-020). `shots/review-queue-call-mode.png`: the wait reason is cut to "disk" and "memory", the rest is a hover title (UXF-024). |
| 4 | Stable layout | fail | `shots/crowded@1366x768-opened.png`, `-loaded.png`, `-next-tick.png`: unit rows open under jobs 100, 104, 108, then 101, 108, 112, then 101, 104, 108 with no input, and the rows below move 30 px (UXF-021, 4 of 11 scenarios). `shots/review-headroom-40-old-daemon.png`: the error line makes the header 37 px taller and the page jumps (UXF-023). Between ticks nothing else moves; the layout shift the round measured (0.088 upgrade-pending, 0.105 machine-headroom) is all in the first 0.6 s of the load, when the header fills and re-wraps. |
| 5 | Provenance of every value | partial | On screen: "20 cores · line: all processes on the host · dashed: pool 20", "(60 min)", "(observe, not enforced)", the headroom caption (`shots/busy-failed@1366x768-loaded.png`), the usage totals (`shots/review-usage-pool-share.png`). Only in hover titles: what "wait p50", "gave up", "PSI cpu", "recommended" and the machine line are computed from (UXF-024). |
| 6 | Errors cheap | pass | `shots/review-headroom-40-video-call.png`: a wrong headroom is one preset click away. `shots/review-headroom-40-old-daemon.png`: a refused change puts the slider back by itself. `ux/out/bot/*.json`: at most 2 actions per task in all 70 runs. |
| 7 | Words the user can say | partial | `shots/machine-headroom@1366x768-loaded.png`: "pool quota 2.0 (floor), outside 9.1: session-3.scope 5.9, docker 3f2a1b2c3d4e 3.2"; every header: "PSI cpu", "recommended 18/20 (observe)", "gave up 0(60 min)" (UXF-028). The tables read well: `shots/review-queue-queue-levels.png` ("#7", "low", "high · waited"). |
| 8 | The counterpart's time respected | pass | The counterpart is the agent whose build waits. `shots/review-headroom-40-video-call.png`: a headroom change is sent 300 ms after the click with no confirmation step, and no control of the page holds a job. `shots/review-queue-queue-levels.png`: the answer for the agent (place 7, level low) is one scroll away. No agent took part, so this is the page's side only. |
| 9 | Keyboard and pointer parity | partial | Every task is done by keyboard at every size (`ux/out/bot/*-keyboard.json`, 0 failed) and the focus ring is plain (`shots/review-tab-26.png`). The cost differs: 26 tabs against 2 pointer actions to job 42 (UXF-026); hover titles and the timeline's bars (`shots/review-timeline-tip.png`) are pointer only (UXF-024). |
| 10 | Recognition over recall | partial | `shots/review-queue-queue-levels.png` names sessions "3d4c5b6a", "7c2e1d4f"; the Sessions table of the same scenario (`shots/queue-levels@1366x768-loaded.png`) names them "docs site redesign", "checkout service: payment retries" (UXF-027). Else good: presets instead of a typed value, "#4" instead of counting rows. |
| 11 | Calm by default | partial | `shots/review-blink-a.png` and `shots/review-blink-b.png`, 500 ms apart with reduced motion requested: the edge of every running bar blinks. `shots/review-crowded-running.png`: 14 red "● live" buttons on a page without a failure (UXF-030). The "not on slot" badge and the "over target" line have cause. |
| 12 | One state vocabulary | partial | `shots/busy-failed@1366x768-loaded.png`: "not on slot", "unmanaged builds" and "bypass slot" for one state. `shots/review-finished-busy-failed.png`, `shots/review-pane-42-keyboard.png`, `shots/review-timeline-tip.png`: "101", "ended (exit 101)", "state failed exit 101" for one result. "live" is a connection, a button and a stream state (UXF-029). |
| 13 | A wrong cue recovered in one action | pass | `shots/review-headroom-40-old-daemon.png`: the page takes the wrong cue back itself (slider at 0 %, caption unchanged) and says why. `shots/review-pane-42-keyboard.png`: a wrong job's pane is replaced by one click or Enter on the right row. `ux/out/bot/*.json`: 0 steers. |
| 14 | Unmistakable end state | pass | `shots/review-pane-42-keyboard.png` and `shots/review-pane-42-960.png`: "ended (exit 101)", the error lines marked, "[exit 101]", the row highlighted. `shots/review-headroom-40-video-call.png`: preset pressed, "40%", "12 of 20 slots", 8 slots drawn dashed. `shots/review-pane-queued-79.png`: "#4" beside job 79. The queued job's pane is the exception, judged under 2. |
| 15 | Readable in every theme; key data at 14 px or more | fail | One theme; axe-core reports no serious finding in any state (`ux/out/gates/*.json`), so contrast holds. Size does not: in `shots/review-pane-42-keyboard.png` the header numbers and table cells are 13 px and the output text 12 px; 15 px only for the title (UXF-031). |
| 16 | Focus follows the work | fail | `shots/review-pane-42-keyboard.png`: after Enter on job 42 the focus ring stays on the job; `shots/review-pane-42-focus-close.png`: "close" is 17 tabs later, and Escape does nothing (UXF-022). By pointer the pane opens beside the row, or under it at 960 px with the row kept in sight (`shots/review-pane-42-960.png`). |

## Findings filed

"Scenarios" replaces "sessions": scenarios affected out of 11.

| id | title | severity | class | scenarios | cost |
|---|---|---|---|---|---|
| UXF-019 | A build that failed a moment ago is not named on the first screen | S2 | cue | 3 of 11 | S |
| UXF-020 | The job tables start below the first screen at every measured size | S2 | layout | 11 of 11 (3 on a task's path) | M |
| UXF-021 | Unit rows open and close by themselves and move the Running rows | S2 | layout | 4 of 11 | S |
| UXF-022 | By keyboard an opened output is 15 tabs from its job, and Escape does not close it | S2 | affordance | 3 of 11 | S |
| UXF-023 | The sticky header takes 15 to 27 % of the screen and grows a row with a status line | S3 | layout | 11 of 11 (2 for the growth) | M |
| UXF-024 | Reasons, sources and full values are only in hover titles | S3 | affordance | 11 of 11 (1 on a card's path) | M |
| UXF-025 | The output of a queued job is an empty pane marked "● live" | S3 | cue | 8 of 11 (4 on a card's path) | S |
| UXF-026 | Reaching a job by keyboard walks every control above it: 26 tabs to job 42 | S3 | affordance | 3 of 11 | S |
| UXF-027 | Job tables name a session by 8 hex digits, the Sessions table by its title | S3 | copy | 10 of 11 | S |
| UXF-028 | The header and the machine line speak in the daemon's terms | S3 | copy | 11 of 11 (1 on a card's path) | S |
| UXF-029 | One state has several names, and "live" has several meanings | S3 | copy | 11 of 11 | S |
| UXF-030 | Running bars blink twice a second, also under reduced motion, and red marks the normal state | S3 | noise | 10 of 11 | S |
| UXF-031 | No text but the page title is 14 px or larger | S3 | layout | 11 of 11 | M |

None of the 18 closed findings has come back. UXF-024 is next to UXF-006 (closed by adding a hover
title) and says so; UXF-023 is not UXF-010 (the panels no longer jump at load; the header does
re-wrap once while it fills).

UXF-020 needs a design decision. UXF-023 and UXF-031 depend on each other: 14 px text wraps the
header a row sooner.

## Not judged

- A lost connection, an output that was not kept, a cancelled job and the "follow newest" switch:
  no scenario plays them. The fake daemon never ends the stream of a job without recorded output,
  so the pane of such a finished job is not evidence (only the queued job's pane is, checked
  against the daemon's handler).
- Hover titles do not render in a screenshot; they were read from the source.
- Heuristic 8 with a real agent waiting, and anything that needs a person: time to understand the
  machine line, whether 13 px is too small on the user's screen.
- The usage table says "no finished jobs in this window" in 10 scenarios because their fixture
  has no usage answer; only pool-share shows it filled.
