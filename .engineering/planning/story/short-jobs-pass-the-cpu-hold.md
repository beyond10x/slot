---
format: aep.planning-md/3
id: story:short-jobs-pass-the-cpu-hold
kind: story
status: draft
title: A job predicted short is not held by the CPU limit for starts
summary: jobs with a p90 under 30 s start without waiting for the CPU hold or its bound
relations:
- serves: vision:O2
revision: 1
---
## Outcome

A queued job predicted short (its command's p90 run time under 30 s, from the job log) is not
held by the CPU limit for starts: it starts as soon as a slot is free, so a check of a few
seconds does not wait for the host to calm down or for the hold's 10-minute bound.

## Why

On 2026-11-23 at about 13:10 CET a docs validator that runs about 3 s waited 616 s in the queue.
In the same minute 20 jobs ran and 43 were queued; the running jobs had waited 543 to 709 s,
which is the CPU hold's bound of 600 s plus its pacing of one start per 30 s. The CPU hold
weighs every job the same: short-jobs-first (`a_job_predicted_short_starts_ahead_of_a_longer_one_of_its_level`)
orders jobs for a slot that frees, but a job the CPU hold keeps waits the bound whatever its
prediction. A 3-second job adds under 1 CPU-second per second of load for 3 s; holding it 10
minutes saves the host nothing.

## Acceptance

- A server test: with the CPU hold active and a job of a command whose recorded p90 is under
  30 s, that job starts at once while a job of a longer command stays held.
- A job with no history is not short: it is held as today.
- The job's record says why it passed (`cpu hold: predicted short, p90 <n> s`).
- The census shows how many jobs passed this way per minute; measured on this host for 30
  minutes: queue wait p90 of jobs under 30 s, before and after.
