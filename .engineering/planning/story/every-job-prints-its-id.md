---
format: aep.planning-md/3
id: story:every-job-prints-its-id
kind: story
status: draft
title: Every job prints its id and node when it ends
relations:
- serves: vision:O6
revision: 1
---
# Story: every-job-prints-its-id

## Outcome

Every `slot` run prints one line on stderr when the job ends: `slot: job <id> on <node> ran <s>s`, adding `waited <s>s for a build slot` when it waited. A job placed on a remote node names that node and the node's job id, so the job log entry can be found from the client's output alone.

## Evidence (2026-11-24, 10:16–10:18 CET)

- Three node runs of one test (exit 101, 101, 0) printed only their sync lines. Their ids (5911, 5930, 5939) had to be looked up in the node's job log by tree, argv and time.
- The daemon prints the job line only when the job waited 2 s or more (`server/mod.rs`, the `note_wait` branch).

## Acceptance

- A job that starts at once ends with `slot: job <id> on local ran <s>s`.
- A job on a remote node ends with the node's name and its id there.
- A job that waited 3 s prints the wait in the same line.
