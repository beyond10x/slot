---
format: aep.planning-md/3
id: story:a-file-a-node-job-deleted-is-deleted-here
kind: story
status: draft
title: A file a node job deleted is deleted here too, when this copy is the one that was sent
relations:
- serves: vision:O6
- informed_by: story:builds-spread-over-two-nodes
revision: 2
---
## Outcome

After a node job, the tree on the client's machine holds what the job left: files the job wrote
come back, and files the job deleted are deleted here too, when this machine's copy is still the
one that was sent. A generator that removes stale pages gives the same tree remotely as locally.

## Why

Measured 2026-11-27 with a probe repository on a build node: an untracked input file reached the
node, and files the job wrote came back, also under an ignored directory. A file the job deleted
(`rm out/stale.txt`) stayed on the client. A consumer whose docs generator writes and removes
generated pages ran its docs steps with `--here` (1.86 slot-hours in under 3 hours) because the
tree after a node run differed from a local run. Today the return sync lives in a host wrapper in
front of `slot`: it lists the node's tree before and after the job and copies back what is new or
changed (`rsync --update`), and never deletes.

## Design

1. The return sync moves into the client (it was planned there in
   `story:builds-spread-over-two-nodes`, design point 2): the listing before and after the job in
   the node's own clock, the copy of new and changed files, the exclusions (`.git`, `target`,
   `node_modules`, a local symlink's path), a local file newer than the node's left alone.
2. Deletion: a path in the listing before the job and missing after it is deleted here when this
   machine's file still has the size and modification time it had when it was sent (the forward
   sync records them), and it is a regular file inside the tree. A directory the job removed is
   removed here when it is empty after that. Nothing outside the tree, under the exclusions, or
   changed here since the sync is deleted; each such case is named on one line.
3. The client prints one line: files back, files deleted, files kept because they changed here.
4. The host wrapper's return sync is removed once the client does it (the wrapper is a host file
   outside this repository; its change is handed to the host's owner).

## Acceptance

- Red first: a stub-node test whose job deletes a file that was sent: before the change the file
  stays here; after it it is gone.
- A file changed here after the sync is kept and named.
- A file the job wrote comes back as before; nothing under `.git`, `target` or `node_modules`
  is touched.
- `website/docs/remote-builds.md` says what comes back and what is deleted.
