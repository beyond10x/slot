---
format: aep.planning-md/3
id: story:node-test-step-ends-within-its-budget
kind: story
status: draft
title: The node test step ends within its budget
relations:
- serves: vision:O2
revision: 2
---
# Story: node-test-step-ends-within-its-budget

## Outcome

`contrib/node/test.sh` ends within the 120 s budget of a repository's test step, and no case in it
runs longer than 60 s. Today it runs provision.sh about 120 times, each a fresh `sh` that reads the
configuration and walks every part; the step is the sum.

## Evidence (2026-11-25)

- One full run, 326 cases, took 656 s with the host at a load average of 73 to 116; the next, 341 cases, took 47 s at a load average of about 20.
- One `provision.sh apply` in test mode took 1.9 s and one `check` 1.2 s in the same minute.
- An earlier run was stopped by the pool's 10 minute limit at 207 cases.

## Acceptance

- `slot --here sh contrib/node/test.sh` ends in under 120 s on a host at its usual load, and
  prints its ten slowest cases.
- The step fails when it runs over 120 s.
- A case that needs a root of its own shares one applied root with the cases that read it, so the
  number of provision runs falls; the cases and their count stay.
