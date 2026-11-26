---
format: aep.planning-md/3
id: story:a-binary-the-node-cannot-load-is-named-by-slot
kind: story
status: draft
title: A binary the node cannot load is named by slot
revision: 1
---
## Outcome

A job on a node that starts a binary the node cannot load ends with one line from slot that
names the binary and the reason, instead of the loader's message somewhere in the job's output.

## Why

A node runs an older C library than the workstation (2.34 against 2.43 where this was seen). A
binary built on the workstation that reaches the node inside a synced tree (a tree's `bin/`, a
tool directory under the home that is mirrored) fails there with `version 'GLIBC_2.xx' not
found`, or with `No such file or directory` for a missing interpreter. The caller reads that as
a defect of the job. One reported case turned out to be a release binary the job downloaded
itself, which only warns (two weak symbols) and runs; no case of a synced binary that fails to
load is on hand yet.

## Acceptance

- Red first: a job on a node that starts a binary asking for a C library version the node does
  not have ends non-zero today with the loader's line only; the test holds that binary as a
  fixture built for the purpose.
- After the change the job's output ends with one line from slot: the path of the binary, the
  version it asks for, the version the node has, and that the binary came with the tree.
- A binary that only warns about a weak version and runs is not reported as a failure.
- The check costs a job that loads every binary no measurable time: the time of a job that
  starts 1,000 processes is measured before and after.

## Notes

- Where the line can come from: the node daemon reading the job's stderr for the loader's two
  message forms at the job's end, or a check of the files a sync brought.
- The tools jobs call by name are installed on the node by `contrib/node/provision.sh` and
  compared with the workstation's by `toolchains.sh`; this story is about binaries outside that
  list.
