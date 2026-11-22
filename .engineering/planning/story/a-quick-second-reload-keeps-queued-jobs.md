---
format: aep.planning-md/3
id: story:a-quick-second-reload-keeps-queued-jobs
kind: story
status: draft
title: A second reload right after a handover keeps every queued job
revision: 1
---
## Outcome

A reload that follows another reload within a second keeps every queued job with its id and its
place in the queue.

## Evidence

2026-11-22, found while tightening the reload test: the first handover carried 20 running and 20
queued jobs; the second, sent right after the first "took over", carried 20 running and 0 queued
jobs and 40 connections (1 of 10 runs beside 20 busy loops). `resume` queues handed-over jobs
again in separate tasks through `run_job`; a reload that comes before those tasks have put the
jobs back in the table hands over their connections without the jobs. The client resubmits a job
whose connection closes before it starts, so the job comes back with a new id at the back of the
queue (inferred from `client.rs`, not checked in that run).

## Acceptance

1. A test reloads twice, the second reload right after the first "took over": the second
   handover carries every queued job, and each keeps its id and `enqueued_ms`.
2. No client resubmits in that test.

## Not in scope

Running jobs: they are adopted before the supervisors start (fixed in wave 2026-11-22-w1).
