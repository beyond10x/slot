# R-02 measurement

Measured after the R-01 change set, at 1366x768, 1280x720, 1920x1080, 1536x754 and 960x1080.
Written by hand from the spec outputs of that run (`round.json` holds the numbers): the run's
`ux/out` was replaced before `node ux/round.ts` existed in its final form, so this round has no
contact sheet.

## Gate problems at the gating sizes

- crowded: not measured. The harness opened the output pane by clicking the row whose cell is named
  "100"; after UXF-001 that cell holds the button "output of job 100", so the click timed out. The
  harness now clicks the button.
- busy-failed, idle, queue-position, video-call, video-call-old-daemon: 0 at 1366x768 and 1280x720
  (R-01: 4 to 8 each).

## Measured-only sizes

- 960x1080: the page scrolls sideways by 50 px in busy-failed, queue-position, video-call and
  video-call-old-daemon (R-01: 386 px).

## Screen-only user

- busy-failed by keyboard: done, 23 tabs (R-01: not reachable).
- busy-failed by pointer: done, 2 actions (scroll, click).
- video-call and video-call-old-daemon: done, 9 actions by pointer, 8 by keyboard (R-01: the same).
- video-call-old-daemon: `false_complete` 0 (R-01: 1).
- queue-position: not done by the bot: it looked for a cell named "79", which is now the button
  "output of job 79". The bot now finds the row by either.

## Layout shift (total over the scenario)

- 1366x768: busy-failed 0.138, idle 0.137, queue-position 0.138, video-call 0.532,
  video-call-old-daemon 0.532.
- 1920x1080: 0.818 to 0.849 in every scenario.

## Not measured this round

- L4 keyed replay, L5 discoverability probe, L6 people: not run (no recorded sessions;
  operator-only). The two review agents (L7) did not run.
