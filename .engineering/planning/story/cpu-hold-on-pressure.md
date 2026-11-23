---
format: aep.planning-md/3
id: story:cpu-hold-on-pressure
kind: story
status: draft
title: The CPU limit for starts also holds on CPU pressure, which keeps rising once the host is saturated
summary: hold queued starts on /proc/pressure/cpu some avg10 as well as on CPU time
relations:
- serves: vision:O2
revision: 1
---
## Outcome

The CPU limit for starts also holds on a signal that keeps rising once the host is saturated:
CPU pressure (`/proc/pressure/cpu`, `some avg10`) or load per CPU. A queued job is held while
that signal is over its mark, and starts again when it falls, so a saturated host takes no new
jobs and an idle host takes them at full speed without an operator moving the mark by hand.

## Why

On 2026-11-23 at 11:46 CET the host ran a 1-minute load of 208 on 20 CPUs, with 33 jobs running,
while `slot cpu-limit 12` held nothing: the pool's CPU figure read 11.5 CPUs. The pool's quota
was 15 cores and the host was saturated (7.5 CPUs outside the pool), so the CPU time the pool
used could not rise past its share however many threads waited. Lowering the mark to 6 brought
the load to 81 within 5 minutes and then held 41 jobs at a load of 24 (12:21 CET), so the mark
had to be moved by hand in both directions. Until this story is built an operator script steps
the mark between 6, 10 and 12 by the load.

## Acceptance

- With a pressure mark set (`slot cpu-limit --pressure P`, kept in `limits.json`), a queued job
  is held while `some avg10` is at or over P and at least the floor's jobs run; it starts when
  the value falls under P. A server test drives the pressure through a stub, as `cpu_busy` does.
- The CPU-time mark and the pressure mark hold independently; `slot cpu-limit` without arguments
  prints both and the current values.
- Held jobs keep the bound of 10 minutes and its pacing of one start per 30 s (0.25.1).
- Off by default; a daemon without the setting writes `limits.json` byte for byte as before.
- Measured on this host for 30 minutes with the setting on: jobs per 30 min, queue wait p90 and
  peak load, against the same window with the operator script.
