---
format: aep.planning-md/3
id: story:a-node-daemon-evicts-the-target-directories-of-its-jobs
kind: story
status: draft
title: A daemon that runs remote jobs evicts their target directories and reports only measured free space
relations:
- decomposes: epic:compute-cluster
- serves: vision:O6
revision: 1
---
## Outcome

A daemon that runs jobs sent to it in remote mode deletes their target directories under the
same rule as a local daemon deletes local ones, and `slot status` never prints a free-space
figure that no look measured.

## Why

On a build node `slot status` printed `disk: 0.0 GB free, under 50.0 GB, raised from 40.0 GB to
the disk hold's mark: deleted 0 target dir(s), 0.0 GB` while every file system had hundreds of GB
free. Two causes in the daemon:

- The eviction look stores 0 bytes as the free space when it knows no target directory that
  exists, and the status line prints that 0 as a measurement.
- A job that arrives with a workspace has an empty `target_dir`; only `node_target_dir` names
  where it built, and eviction reads `target_dir` alone. A daemon that only runs such jobs knows
  no target directory, so nothing is ever deleted there and the disk only grows (461 GB used on
  the node where this was seen).

## Acceptance

- With no known target directory, `slot status` says that nothing was measured and names the
  mark; a newer client that reads an older daemon's report prints what it printed before.
- A finished job that arrived with a workspace makes its node target directory known to
  eviction; when free space is under the mark the directory is deleted under the idle rule, and
  a directory a running job uses is kept.
- Red first: a test of the status line and two tests of the server fail before the change.
- The node gets the change through a release and the version pin of `contrib/node/provision.sh`.

## Notes

- Trees of worktrees that are finished on the workstation stay on the node; that is a separate
  question from the target directories.
