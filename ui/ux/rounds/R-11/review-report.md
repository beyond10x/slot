# R-11 report review

Verdict: **not fit to send as is.**

Default action: send `report.md` with § 4 in front of it, do not run `node ux/ratchet.ts --round
--update` yet, and fix T1 to T3 before R-12. The report renders `ux/out` faithfully, but it does not
show the one failing check of the round (the round ratchet: exit 1, 10 failures), one metric is not
what its name says in 11 of 11 rows, and two of the three new scenarios are covered less than their
rows suggest.

Inputs: `ui/ux/out/gates/*.json` (55 files), `ui/ux/out/bot/*.json` (70 files),
`ui/ux/out/gates/shots/*.png` (145 files), the tool sources, `baseline.json`, R-09 and R-10. Nothing
was run on the worktree; the tool demonstrations D0 to D12 ran on copies of `ux/` in a scratch
directory (scripts `recompute.mjs`, `botfacts.mjs`, `history.mjs`, `demos.sh`, `demo-stale.sh`, not
in the repository). No Playwright run, so every statement below about a cause that needs a second
measurement says so.

## 1. Are the numbers right and meaningful?

### 1.1 Recomputed from the raw files

| number | report | recomputed | differs |
|---|---|---|---|
| gate problems at 1366x768 and 1280x720 | 0 | 0 (22 files, 58 states, recounted from the probes) | no |
| problems at the three measured-only sizes | 0 | 0 (33 files, 87 states) | no |
| screen-only runs | 70 lines, all done | 70 files, 70 done, 0 `completion_miss`, 0 `false_complete`, 0 notes | no |
| trend row | 0, 0, 0 | 0, 0, 0 | no |
| metric rows at 1366x768 | 100 | 89 equal, 11 differ | `layout_shift.per_minute`, all 11 |
| contact sheet | 29 links | 29 files at 1366x768 in `ux/out` | no |

Completeness: 11 scenarios × 5 sizes = 55 gate files, 7 tasks × 5 sizes × 2 modes = 70 bot files,
all written in one run of 6 min 56 s. No file is missing or stale in this round. D0: the round
command on an unchanged copy writes a byte-identical `round.json`.

`layout_shift.per_minute` is the raw shift total, not a rate. `metrics.ts:62` divides the total by
`Math.max(1, g.minutes)`; every scenario lasts 1 to 6 simulated seconds, so the divisor is always 1.
The gate files hold a rate of their own (`layout_shift.per_minute`), which the report does not use:

| scenario | report | raw total | simulated s | per minute in the gate file | report is lower by |
|---|---|---|---|---|---|
| busy-failed | 0.035 | 0.0347 | 6 | 0.347 | 0.312 |
| call-mode | 0.028 | 0.0279 | 1 | 1.671 | 1.643 |
| crowded | 0.044 | 0.0441 | 2 | 1.322 | 1.278 |
| idle | 0.035 | 0.0347 | 1 | 2.083 | 2.048 |
| machine-headroom | 0.105 | 0.1047 | 2 | 3.142 | 3.037 |
| pool-share | 0.035 | 0.0347 | 6 | 0.347 | 0.312 |
| queue-levels | 0.035 | 0.0352 | 1 | 2.111 | 2.076 |
| queue-position | 0.035 | 0.0347 | 1 | 2.082 | 2.047 |
| upgrade-pending | 0.088 | 0.0882 | 2 | 2.646 | 2.558 |
| video-call | 0.035 | 0.0349 | 1 | 2.091 | 2.056 |
| video-call-old-daemon | 0.035 | 0.0349 | 1 | 2.091 | 2.056 |

Neither column is a per-minute rate of anything a user sees. The observer is buffered from page load
and the gate spec never scrolls, so the total is the shift while the page loads: busy-failed (3
state changes) and idle (1) both read 0.0347. Dividing that by the scenario's simulated length ranks
scenarios by their step count (busy-failed 0.347, idle 2.083 for the same total). The honest name of
the reported value is "layout shift per page load". Read under that name, all 11 values are right.

### 1.2 The round ratchet: 10 failures, none shown

`node ux/ratchet.ts --round` exits 1. `report.md` has no line about it; the round task does not run
it (T3).

| failure | meaning | in report.md |
|---|---|---|
| upgrade-pending `layout_shift.per_minute`: 0.088 against 0.005 (tolerance 0.01) | worse by 0.083. R-09 0.088, R-10 0.005, R-11 0.088; the `opened` screenshot is byte-identical in the three rounds; by `R-10/triage.md` the only change between the R-09 and R-10 measurements was the bottom-sheet scroll. The baseline took 0.005, a value below every other scenario since R-03 (lowest 0.028). So the baseline is probably one low sample of a load race, and 0.088 the usual value; a regression since R-10 is not excluded (`HeaderBar.vue` changed). Three repeated measurements of this scenario decide it. | the row reads `0.088 \| 0.005 \| 0.005`, unmarked |
| machine-headroom, pool-share, queue-levels × `reading_load.max`, `reading_load.median`, `layout_shift.per_minute` (9): measured but not in the baseline | not regressions: the three scenarios came in by merges after R-10, and `--update` in merge mode never takes host-dependent keys in (T3, D8). The 9 values have no ratchet. | `–` in the baseline column, 9 rows, unmarked |

Within this run the measurement repeats: busy-failed and pool-share load the same page and agree to
4 decimals at 5 of 5 sizes, as do video-call and video-call-old-daemon. So 0.088 is a property of
this build, not noise of this run.

### 1.3 Does each metric mean what its name says?

| key | this round | judgement |
|---|---|---|
| `gates.problems` | 0 in 11 scenarios | Means it: problems summed over both gating sizes and all states. A gate file that is missing counts as 0 (T1, T4). |
| `reading_load.max`, `.median` | 202 in 7 of 11 scenarios | Words of DOM text whose box touches the first viewport. Not occlusion-checked, and blind to the 4 canvas charts (axis and timeline-bar labels). The gate never scrolls: 7 scenarios with different jobs and queues read 202 because the first 768 px hold the header, the charts and the Sessions table, which the fixtures share. The job tables that the scenarios vary are below the fold and unmeasured. `max` equals `median` in 11 of 11 scenarios (2 to 4 near-equal states), so `median` carries nothing. |
| `layout_shift.per_minute` | § 1.1 | Not a rate; the load shift. Shifts in the job tables (rows inserted by the steps) are below the fold and not counted. |
| `screen_only.actions` | 0 to 2 | Means it, pointer run only. A scroll into view counts as 1 action. |
| `actions_per_fix.max` | equals `screen_only.actions` in 7 of 7 | A copy (`metrics.ts:72`, `:76`); no maximum is taken, and 2 of 7 tasks have 0 actions and fix nothing. |
| `screen_only.looks` | 1 or 3 | Not a count: the glance grade (1 header, 2 in view, 3 not in view) of the one target. |
| `screen_only.time_to_act_steps.max` | 1 iff looks = 3, in 35 of 35 pointer runs | Not a time and not a maximum: 1 when one scroll was needed. Third key for the same fact. |
| `screen_only.steers` | 0 in 70 of 70 | Never written (`bot.ts:52` only). A constant, ratcheted in 7 baselines. |
| `screen_only.failed` | 0 | Means it, pointer run at 1366x768 only. |
| `screen_only.keyboard_failed`, `.keyboard_tabs` | 0 failures; tabs 26, 4, 4, 0, 0, 0, 0 | Mean it for 3 scenarios (busy-failed, both video-call). For queue-levels, queue-position, upgrade-pending and machine-headroom the keyboard run presses no key: 20 of 35 keyboard files equal their pointer file in every field (T6). Their `0` rows are not measurements. |

Comparability with R-10: 8 scenarios are comparable (same harness, font and sizes); 3 are new and
have no last-round value. `layout_shift` of upgrade-pending is not comparable until re-measured.

### 1.4 Measured, but in no report, trend or ratchet

- Reading load and layout shift at 1280x720 (a gating size), 1536x754, 1920x1080 and 960x1080: 44 of
  55 gate files. The round's largest values are there: layout shift 0.122 (crowded, 960x1080), 0.115
  (crowded, 1920x1080), 0.111 (upgrade-pending, 1536x754; machine-headroom, 960x1080); reading load
  870 (crowded, 960x1080) and 803 (1920x1080) against 559 in the table. D7: ten times the shift at
  1920x1080 and twice the words at 1280x720 leave `round.json` byte-identical.
- Screen-only results away from 1366x768 (56 of 70 files): listed line by line, absent from the
  metrics, the trend and the ratchet (D5). In this round the 5 sizes agree in every field in 14 of
  14 scenario-and-mode groups.
- Problems at the measured-only sizes: listed, not in `round.json`, the trend or any exit code (D6).
- call-mode, crowded, idle and pool-share have no task: no screen-only data for 4 of 11 scenarios.
- The page after the bot acted (output open, headroom at 40 %) is in no screenshot; only crowded
  opens the pane in the gate spec. 116 of 145 shots stay in `ux/out`, which is not committed.

## 2. Findings the report misses

1. **pool-share measures nothing of its subject.** Its 20 screenshots are byte-identical to
   busy-failed's (5 sizes × 4 states) and every number is equal. The usage table is outside the
   first screen at every size (up to 1080 px high), the scenario has no task although its card is a reading task ("Read how the
   slot time was shared"), and the contact sheet shows four busy-failed pictures under its name.
   Only the whole-document probes (scrollers, clipped text, panel overflow, axe) touch the table.
2. **Every task on a job needs a scroll, at every size.** busy-failed, queue-levels and
   queue-position: look grade 3 in 15 of 15 pointer runs, including 1920x1080 and 960x1080. At 1080
   px of height the queue and job tables are still below the fold
   (`ux/out/gates/shots/queue-position@1920x1080-loaded.png`). busy-failed sits at the limit of 2
   actions at every size, and takes 26 Tab presses by keyboard. The report prints these as numbers
   equal to the baseline.
3. **machine-headroom has the largest load shift at the gating sizes** (0.105 at 1366x768, 0.108 at
   1280x720, three times the 0.035 of 7 scenarios) and a header of five rows at 1366x768, about 195
   of 768 px (`shots/machine-headroom@1366x768-loaded.png`). The row has no baseline and no mark.
4. **crowded reading load rose from 548 to 559** (2 %, inside the tolerance of 55), the first move
   since R-07. One of 3 values that changed since R-10; the report has no list of changes, the
   reader finds them among 100 rows.
5. **A closed finding's regression asset is failing.** UXF-010 (S3, closed) names "`ux/baseline.json`
   `layout_shift.per_minute` (round ratchet)". That ratchet fails on this key and no task runs it;
   `findings.test.ts` checks only that `ux/baseline.json` exists.
6. **The new header regions are outside gate 2.** `machine` and `upgrade` carry `data-ux-region`
   marks; the always-visible list has 4 of the 6 marks (T8).

## 3. Tool defects

| id | defect | file and line | input that shows it |
|---|---|---|---|
| T1 | A test that crashes before its last statement writes no file, and the round reads the gap as a pass: "None.", trend `0 \| 0 \| 0`, exit 0. A click on a covered control times out and is such a crash. | `Taskfile.yml:37` (`\|\| true`); `gates.spec.ts:51`, `screen-only.spec.ts:26` (write after the body); `metrics.ts:36-39`; `round.ts:37`, `:77-78`, `:81` | D1: no `ux/out` at all gives a report with "None." twice, an empty table and trend `0 \| 0 \| 0`. D2: without `crowded@1280x720.json` and both busy-failed bot files at 1366x768, the report drops 7 rows and names nothing missing. The round knows the expected set (scenarios × sizes × modes) and does not compare; the R-02 trend row carries a hand note for such a gap ("crowded not measured"). |
| T2 | `layout_shift.per_minute` is the total; the clamp makes the divisor 1 below 60 simulated seconds. | `metrics.ts:62`; `gates.spec.ts:44-47` | § 1.1. D9: the same total over 1 s and over 60 s gives 0.035 both times. |
| T3 | The host-dependent keys are ratcheted by no task. The merge ratchet skips them, the round task does not call `ratchet.ts --round`, and `--update` in merge mode never adds them for a new scenario. | `ratchet-lib.ts:56`, `:75`, `:84`; `ratchet.ts:16`, `:38`; `Taskfile.yml:27`, `:36-38` | The 10 failures of this round. D8: a baseline without machine-headroom, then `ratchet.ts --update`: 9 keys taken in, exit 0, merge ratchet green, `--round` still fails on 3. D11: 2000 words and a shift of 5 at 1366x768 pass the merge ratchet. |
| T4 | The merge ratchet passes with gate files of a gating size missing: `gates.problems` is summed over whichever gating files exist. | `metrics.ts:57-58` | D3: every `*@1366x768.json` gate file removed, merge ratchet exit 0. D4: every 1280x720 file removed, both ratchets exit 0 and name nothing. |
| T5 | Trend, `round.json` and the ratchet read screen-only results and the host-dependent keys at 1366x768 only; a failure at another size, 1280x720 included, reaches only its report line. The console line counts `!done` at all sizes, the trend counts three flags at one size. | `metrics.ts:58`, `:65`; `round.ts:77-78`, `:81` | D5: NOT DONE at 1280x720 and `false_complete` at 960x1080 give trend `0 \| 0 \| 0` and a green merge ratchet. D6, D7. |
| T6 | Keyboard mode uses no key for `see` and `queue-position` tasks; it scrolls with `scrollIntoViewIfNeeded` like the pointer. | `bot.ts:151-165`, `:167-185` | 20 of 35 keyboard files equal the pointer file in every field. |
| T7 | Keys that cannot move or repeat another key: `steers` (never written), `actions_per_fix.max` (copy), `time_to_act_steps.max` (function of `looks`), `reading_load.median` (equals `max`). | `bot.ts:52`, `:70-74`; `metrics.ts:61`, `:72-76` | § 1.3: 70 of 70, 7 of 7, 35 of 35, 11 of 11. |
| T8 | The always-visible list misses the `machine` and `upgrade` region marks. | `probes.ts:6-11`; `src/components/HeaderBar.vue:90`, `:93` | 6 marks in the source, 4 in the list. |
| T9 | The gate spec never scrolls and opens the pane only for crowded, so viewport-bound probes (words, shift, always-visible) and the screenshots see only the first screen. A scenario whose subject is lower measures nothing new. | `gates.spec.ts:29-41`; `probes.ts:76-90` | pool-share against busy-failed: 20 of 20 shots identical. 202 words in 7 scenarios. |
| T10 | A `see` task is done as soon as the text is in the DOM; it is not checked again after the scroll, and the truth is one substring of a card that asks two things (machine-headroom: "whether slot is holding builds back and what uses the rest"). | `bot.ts:152-164`; `scenarios.ts:325` | machine-headroom: done with 0 actions on "over target". |
| T11 | The merge task does not clear `ux/out`; a stale file of a removed scenario keeps a local ratchet green where a clean checkout fails. | `Taskfile.yml:26-27` | D12: baseline entry for a scenario no spec writes: exit 1 clean, exit 0 with the stale file. |
| T12 | Report shape: no mark on a changed or unbaselined row and no ratchet result; "Not measured this round" is fixed text; the screen-only list sorts video-call-old-daemon before video-call and 960 after 1920; `rounds/R-nn/shots` is never cleared before the copy; a re-run rewrites the round's trend row and drops hand notes such as R-02's; `anchor` is read by no code. | `round.ts:42`, `:46-58`, `:60-66`, `:79-80`; `scenarios.ts:25` | this report; the R-02 trend row. |

Answers to the questions asked:

- Stale or missing file read as a pass: missing, yes (T1, T4). Stale, not in the round (the task
  removes `ux/out` first), yes in the merge task on a developer's tree (T11).
- Merge ratchet with a value missing: fails for a count key that is absent altogether; passes when a
  gating gate file is absent (T4) and for every host-dependent key (T3).
- New scenario with no baseline entry: the merge ratchet fails on its count keys (D8, 9 failures),
  and passes on its 3 host-dependent keys for good.

## 4. Caveats to send with the round

1. No person, one theme, one run per scenario and size; 125 hermetic tests, one result file each.
2. `node ux/ratchet.ts --round` fails with 10 lines. One is a worse value: upgrade-pending layout
   shift 0.088 against a baseline of 0.005, which is probably a baseline taken from one low sample
   (R-09 read 0.088 on the same pixels); re-measure three times before changing the baseline or the
   page. Nine are values of the three new scenarios that have no baseline yet.
3. `layout_shift.per_minute` is the layout shift of one page load, not a rate. machine-headroom has
   the highest (0.105).
4. All table values are at 1366x768. Reading load and layout shift at the other four sizes are in
   no table; the largest of the round are at 960x1080 and 1920x1080 (§ 1.4).
5. Reading load and layout shift cover the first screen only. pool-share adds no measurement of the
   usage table; its pictures are busy-failed's.
6. "by keyboard: done" is a keyboard result for busy-failed and the two video-call scenarios only.
7. `screen_only.steers` is a constant 0; `actions_per_fix.max` and `time_to_act_steps.max` repeat
   `screen_only.actions` and `screen_only.looks`.
8. Outside this review's questions, an estimate and not a measurement: at this round's rate (125
   tests in 6 min 56 s with 2 workers) the 50 tests of the merge gate take about 2 min 45 s.
