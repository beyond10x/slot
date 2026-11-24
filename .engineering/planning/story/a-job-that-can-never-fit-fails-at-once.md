---
format: aep.planning-md/3
id: story:a-job-that-can-never-fit-fails-at-once
kind: story
status: draft
title: A job predicted to need more memory than the host has fails at once with a line
relations:
- serves: vision:O2
- informed_by: story:admission-waits-for-memory
revision: 1
---
# Story: a-job-that-can-never-fit-fails-at-once

## Outcome

A job whose predicted memory need is more than the host could ever offer (total memory minus the
floor that stays free) is not queued. It ends at once with exit 3 and one line:
`slot: this job is predicted to need <need> (peak of job <id>); this host offers at most <max>`,
and how to run it anyway (`--memory-max`, which replaces the prediction with a bound).
A single runaway peak does not set the prediction for every later job with the same key.

## Evidence (2026-11-24, a 124 GB build node)

- One `cargo nextest run -E all()` in a mutation-testing copy ran 67 s and recorded `peak_rss`
  123.4 GB with 68 CPU-seconds and exit 100: a mutant that allocated until the host was full.
- The next job with the same key (repository directory `copy` and the same argv) was queued for
  29 minutes and more with "the job needs about 123.4 GB and 4.0 GB stays free". It could never start.
- Every mutation-testing copy is named `copy`, so all mutants of all repositories share one key.

## Recommendation

- Admission refuses a request whose need exceeds `total − floor` at submit time, with the line above.
- The prediction uses the median of the last 5 peaks of the key, not the maximum, and a peak from a
  job that failed is kept apart from the peaks of jobs that passed.
- The key includes the checkout's origin (the repository of the tree a copy was made from), not only
  the directory name.

## Acceptance

- A job whose prediction exceeds `total − floor` exits 3 within 1 s with the line; nothing is queued.
- After one failed job with a 120 GB peak and four passed jobs with 2 GB peaks of the same key,
  the next job is predicted at about 2 GB and starts.
