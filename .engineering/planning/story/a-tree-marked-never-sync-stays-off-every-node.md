---
format: aep.planning-md/3
id: story:a-tree-marked-never-sync-stays-off-every-node
kind: story
status: draft
title: A tree marked never-sync stays off every build node
relations:
- serves: vision:O2
- decomposes: epic:compute-cluster
revision: 4
---
# Story: a-tree-marked-never-sync-stays-off-every-node

## Outcome

Rule: a tree that holds customer data, message content or production configuration is never synced to a build node; the `.slot/never-sync` file in the tree is the durable form, so the repository carries it.

A tree whose data must not leave the host (customer data, private records) is never synced to a build node. It is marked in one of two ways: a path prefix in the user's slot configuration, or a file `.slot/never-sync` at the tree's root, committed with the tree so every clone carries it. Its jobs run in the host's pool, and `slot status` and the job record show the reason `never sync`.

## Evidence (2026-11-24, 10:26 CET)

- A Python test step in a repository holding customer data was sent to a build node: the sync started before its owner stopped it.
- The node already held a 469 MB copy of the tree (10,609 files) from an earlier sync the evening before; it was deleted by hand at 10:31 CET.
- Four more trees with message content, security review findings and production configuration were found on the node and deleted at 10:33 CET (about 5.0 GB, 74,555 files).
- 10:39 CET: a job started from the home directory (an empty worktree root made `cd` fall back to `$HOME`) tried to sync the whole home directory; rsync failed at once and nothing was written. A tree root equal to the home directory, `/home` or `/` now runs on the host with the reason `never sync: home directory`.

## Acceptance

- With either mark, `slot run -- <cmd>` in the tree or a subdirectory runs on the host, sends no byte to a node, and the job's `local_reason` reads `never sync`.
- A compile in such a tree runs on the host and is not refused by the scratch or `--here` rules.
- No node's mirror directory ever holds the tree: a test with a fake remote counts 0 rsync calls.
- A job whose tree root is the home directory, `/home` or `/` (no git repository above the working directory) runs on the host with the reason `never sync: home directory` and sends no byte to a node; a test runs `slot run -- true` from the home directory against a fake remote and counts 0 rsync calls.
