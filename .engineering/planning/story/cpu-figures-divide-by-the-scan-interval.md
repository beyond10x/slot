---
format: aep.planning-md/3
id: story:cpu-figures-divide-by-the-scan-interval
kind: story
status: draft
title: No CPU figure of the daemon reads more than the host has
relations:
- serves: vision:O6
revision: 1
---
## Outcome

No CPU figure of the daemon reads more than the host has.

## Context

- The census divided the CPU time between two `/proc` scans by the time between two ticks of the
  sampler's 1-second clock (`crates/slot/src/server/mod.rs`, `sampler`). The scan runs on a
  blocking thread after the tick.
- Measured 2026-11-22 in `census.jsonl` over 6 hours: 35 of 2,160 records read over 20 CPUs on a
  host of 20, the highest 37.0 at 14:40:08, all at a load of 108 or more (6-hour median 69). The
  pool's classes alone read 22.6 at 17:13:42. The web view scaled its chart to these ("30 cores").
- Cause inferred from the code; a 25 s probe at load 40 did not reproduce an overshoot.

## Acceptance

1. Every CPU figure of a sample divides by the time between the two scans; a failed scan does not
   shorten the next interval (unit test `a_cpu_sample_covers_the_time_between_two_scans_not_two_ticks`).
2. After the release is live, the census records of 6 hours with a load over 100 have no
   `host_cores` over the CPU count (query quoted).

## Note

Built before this record existed, outside the wave of 2026-11-22.
