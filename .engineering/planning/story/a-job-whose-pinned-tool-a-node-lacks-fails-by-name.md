---
format: aep.planning-md/3
id: story:a-job-whose-pinned-tool-a-node-lacks-fails-by-name
kind: story
status: draft
title: A job whose pinned tool version a node lacks fails with a line naming the tool and both versions
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
revision: 1
---
# Story: a-job-whose-pinned-tool-a-node-lacks-fails-by-name

## Outcome

A job placed on a remote node runs only with the tool versions its tree pins. When the tree pins a
version of a tool (a rust toolchain in `rust-toolchain.toml`, a CLI version in a tree's tool file
such as an `*_VERSION` line the tree's gate checks) and the node does not have that version, the
job does not run there: it fails at once with one line naming the tool, the pinned version and the
version the node has. It never runs another version without saying so.

## Evidence (2026-11-24, one hand-started build node)

- 05:00–05:17 CET: build-script results built by one rust toolchain were reused by another on the
  node; jobs failed later in the build with errors that named neither toolchain. Fixed by hand
  (toolchain mirror, build-script results deleted), with no check in slot.
- About 08:00 CET: a tree pinned a specification CLI at 0.97.4 (its CI installs that release); the
  node's `PATH` resolved the same CLI to 0.98.0, and the job checked the tree with the wrong version
  and exited 0. The pinned release was on the node under a versioned file name, but nothing chose it.

## Recommendation

- A node reports, with its health answer, the version of each tool a registered toolchain names
  (rust toolchains with their targets, go, node, and tools a toolchain declares by name and
  version file).
- Before placement, the client reads the tree's pins through the same toolchain types
  (`Toolchain`, `toolchain::registry`) and compares them with the node's report.
- A missing or different version: the job ends with exit 3 and
  `slot: <tool> <pinned> is pinned by this tree; the node has <version>`, before any sync.
  Placement on the local node follows the cluster's rules for a job that cannot run remotely.
- A version present under a versioned name (`<tool>-<version>`) is put first on the job's `PATH`
  on the node, so the pinned version is the one the job resolves.

## Acceptance

- A tree pinning a tool version the node lacks: the job exits 3 with the line above, and the
  node's job log has no entry for it.
- A tree pinning a version the node has under `<tool>-<version>`: `<tool> --version` inside the job
  prints the pinned version.
- A tree pinning nothing: placement is unchanged.
