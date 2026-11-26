---
format: aep.planning-md/3
id: story:a-seeded-target-never-makes-a-changed-source-look-built
kind: story
status: active
title: A seeded target directory never makes a changed source look built
relations:
- serves: vision:O6
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-24T16:21:06Z", actor: "agent:slot", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-24T16:21:06Z", actor: "agent:slot", revision: 3}
---
# A seeded target directory never makes a changed source look built

## Why

On 2026-11-24 a remote `cargo test` failed on the build node with errors about a field that the
synced source file held. The tree's files on the node matched the workstation's, but the node had
seeded the tree's `target/` from a sibling worktree by hard links after the sync: the seeded
fingerprint of the changed crate was newer than the synced source file, so cargo, which decides by
mtime, used the sibling's build of the old source. Touching the changed files afterwards did not
make cargo rebuild it. Hard links also let a sibling's later build and this tree's build share
files.

## Acceptance

- A seeded target directory is a copy (reflink or byte copy), never hard links into a sibling's
  target.
- After seeding, every file of the tree whose content differs from the sibling's copy is newer than
  every file of the seeded target, so cargo rebuilds each crate that holds one.
- A remote job's record names the sibling it was seeded from (`seeded_from`, built).
- Red first: two worktrees of one repository, the second changes a crate's source and is seeded
  from the first after its sync; a remote build of the second compiles the changed crate.
