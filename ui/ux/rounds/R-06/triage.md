# R-06 triage

## Caveats

- Measured after moving the branch onto 0.5.0, which added the call badge in the header and the
  wait reason in the Queue table. A new scenario, call-mode, holds both (headroom 40 % held by a
  call, two jobs waiting for disk space and memory).
- No person, no review agents, one theme; as in R-01.

## Did the last change sets move their metrics?

No change set since R-05. call-mode: 0 gate problems at 1366x768 and 1280x720.

## Findings and priority

| finding | severity | frequency | confidence | cost | priority |
|---|---|---|---|---|---|
| UXF-015 job tables under the pane at 960x1080 (measured-only) | S3 (1) | 1/7 | 1 | M (2) | 0.07 |

## Change set R-06

Empty. The loop stays stopped: the gates pass with 0.5.0's header and queue; UXF-015 is S3 at a
size the gates do not hold.
