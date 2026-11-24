---
format: aep.planning-md/3
id: story:a-jobs-width-follows-the-nodes-free-cores
kind: story
status: active
title: A job's width follows the free cores of the node it runs on
relations:
- serves: vision:O6
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-24T13:09:05Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-24T13:09:05Z", actor: "human:timo", revision: 3}
---
# A job's width follows the free cores of the node it runs on

## Why

On 2026-11-24, 12:19-13:19 CET, a 64-CPU build node ran on average 6.75 jobs at 3.48 cores
each; 81 % of job wall time ran while more than 30 cores were idle. slot sets 16 test threads
on every job and nextest peaked at 15.5 cores. Sessions pass worker flags written for a 20-CPU
workstation (`--workers=2` for Playwright, `--maxWorkers=4` for Vitest, `NEXTEST_TEST_THREADS=4`).

## Acceptance

- slot gives a job a width when it starts: the node's free cores at that moment, divided by the
  jobs that started in the last few seconds, between 1 and the node's CPUs.
- slot exports it to the job: `RUST_TEST_THREADS`, `NEXTEST_TEST_THREADS`, the cargo jobserver
  tokens, Go's `-p` and `GOMAXPROCS`, and a `SLOT_WIDTH` that Playwright and Vitest configs read;
  a worker flag the caller passed that is lower is raised on a node and kept on a workstation.
- The workstation rule stays for local jobs.
- The 16-thread cap on tests no longer applies on a node.
- Red first: a job started on an idle 64-CPU node in a test gets more than 16 test threads; two
  jobs started at once split the free cores.
- Measured after: the node's busy cores and jobs per hour over one hour, against 18.2 cores and
  1,085 jobs (12:19-13:19 CET, 2026-11-24).
