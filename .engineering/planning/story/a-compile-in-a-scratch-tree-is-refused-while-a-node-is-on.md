---
format: aep.planning-md/3
id: story:a-compile-in-a-scratch-tree-is-refused-while-a-node-is-on
kind: story
status: draft
title: A compile in a scratch tree is refused while a build node is on
relations:
- serves: vision:O2
revision: 1
---
# Story: a-compile-in-a-scratch-tree-is-refused-while-a-node-is-on

## Outcome

While a build node is configured, a job whose tree is under the cache directory (session scratch, which stays on the host) and whose command is cargo, rustc or go is refused with exit 3 and the line `move the tree into a managed worktree and run it there`. Other commands under the cache directory still run on the host.

## Evidence (2026-11-24)

- 139 jobs ran on the host under the cache rule in 24 h, from 11 sessions; 94 were cargo.
- One of them, a cold build at 1 slot, used 4.1 CPUs on average (story:a-jobs-slots-bound-the-compiles-the-cache-server-runs-for-it).

## Acceptance

- From a tree under the cache directory: `slot run -- cargo test` and `slot --timeout 5m cargo build` exit 3 with the line; `slot run -- sh probe.sh` and `slot --local run -- cargo build` run.
