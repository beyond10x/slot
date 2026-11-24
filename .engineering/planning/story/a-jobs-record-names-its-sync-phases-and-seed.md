---
format: aep.planning-md/3
id: story:a-jobs-record-names-its-sync-phases-and-seed
kind: story
status: active
title: A job's record names its sync, its phases and its seed
relations:
- serves: vision:O6
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-24T13:09:05Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-24T13:09:05Z", actor: "human:timo", revision: 3}
---
# A job's record names its sync, its phases and its seed

## Why

A measurement of one hour of a build node (2026-11-24, 1,085 jobs) could not answer three
questions from the job log: how long a job spent syncing its tree to the node and its files back,
how a `cargo test` job split between compiling and running tests, and whether a node job's target
directory started empty or seeded from a sibling. The client prints the sync line on stderr only;
`target_dir` is empty for every remote cargo job.

## Acceptance

- A remote job's record in the client's job log and in the node's carries `sync_ms` (tree to the
  node), `back_ms` (products and fetches back) and `sync_bytes`.
- A cargo `test`, `nextest` or `bench` job carries `compile_ms` and `test_ms`, split where cargo's
  JSON messages show the last `compiler-artifact` before the first test binary runs.
- A remote cargo job carries its node target directory and `seeded_from` (the sibling it was
  seeded from, or `empty`, or `kept`).
- Every new field has `#[serde(default)]`; an older record reads as unknown, never 0.
- Red first: a job log line of the old shape fails a test that asks for each field.
