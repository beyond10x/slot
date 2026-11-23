---
format: aep.planning-md/3
id: story:new-worktree-seeded-from-a-sibling
kind: story
status: draft
title: A new worktree's mirror on a node is seeded from a sibling, so the first sync sends deltas
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
- depends_on: story:placement-sticks-to-a-node
- depends_on: story:sync-sends-the-git-file-list
- depends_on: story:one-hand-started-node-measured
- depends_on: story:nodes-start-on-queue-pressure
scope:
- confidence: inferred
  path: crates/slot/src/client.rs
- confidence: inferred
  path: crates/slot/src/server/cache.rs
- confidence: inferred
  path: crates/slot/src/server/mod.rs
- confidence: inferred
  path: crates/slot/src/shell.rs
revision: 3
---
# Story: a new worktree's mirror is seeded from a sibling on its node

## Outcome

The first sync of a new worktree to a node that holds a mirror of the same repository sends only
the difference: the node makes the new mirror a reflink copy of the newest sibling mirror first,
then rsync sends deltas. A seeded target directory holds dependency artifacts only, so no build
reports fresh what another commit built.

## Specification

`spec/domains/cluster.yaml` Placement (`repository`), PlaceWorktree.

## Design

`docs/design/remote-pool.md` § 4.7 (seed the mirror before the first sync; today the sync comes
first), § 4.5 (seeding a target directory across commits; drop the workspace's own
fingerprints), § 4.6 (first sync sizes), `server/cache.rs` seeding. The phase-0 report of
`story:one-hand-started-node-measured` says whether today's seeding showed the defect; this story's
checks hold either way.

## Acceptance

1. On a stand-in node holding a mirror of repository R, the first sync of a new worktree of R at
   the same commit transfers under 5 % of the tree's bytes (rsync `--stats`), against 100 %
   without a sibling.
2. The client asks the node to seed before it syncs; a node of an older version that does not
   know the step gets a plain sync.
3. A target directory seeded for a new worktree holds no fingerprint directory of the
   repository's own packages (the test lists `.fingerprint/<package>-*` for each workspace
   member and finds none), and the dependency fingerprints are there.
4. Two worktrees of one repository at different commits on one node: after seeding, the second's
   first build compiles every workspace crate whose sources differ (cargo's `--message-format
   json` lists each as `compiler-artifact` with `fresh: false`).
5. Tests: no network; each under 60 s, the step under 120 s.

## Scope

`crates/slot/src/client.rs`, `crates/slot/src/shell.rs`, `crates/slot/src/server/cache.rs`,
`crates/slot/src/server/mod.rs`.
