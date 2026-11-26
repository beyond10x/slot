---
format: aep.planning-md/3
id: story:products-copied-back-do-not-fill-a-target
kind: story
status: active
title: Products copied back from a node do not fill a tree's target directory
relations:
- serves: vision:O6
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-24T15:37:55Z", actor: "agent:slot", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-24T15:37:55Z", actor: "agent:slot", revision: 3}
---
# Products copied back from a node do not fill a tree's target directory

## Why

After a remote cargo test, the client copies the workspace's test and bin binaries back into the local target directory. Each rebuild of a test target writes a binary under a new hash name, and old ones are never removed; on 2026-11-24 a worktree's private `target/` reached 14 GB.

## Acceptance

- After copying a product back, the client removes older files of the same target (same name stem, other hash) from the local `deps/` directory.
- `SLOT_NO_PRODUCTS=1` turns the copy-back off for a job; documented in `reference/environment.md`.
- Red first: two remote test runs of a changed test target leave one test binary of it in the local `deps/`.
