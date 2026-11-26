---
format: aep.planning-md/3
id: story:only-files-a-node-job-wrote-come-back
kind: story
status: draft
title: Only the files a node job wrote come back into the session's tree
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- depends_on: story:results-come-back-from-a-node
revision: 1
---
## Outcome

After a job on a node, the session's tree gets the files that job wrote and no other file. A file
the session deleted or renamed before the job stays gone, whatever the node's copy of the tree
still holds.

## Evidence (2026-11-26)

- A session renamed a tracked test file with `git mv`. `git status --short` showed one `R` line.
  Three node jobs later the old name was back as an untracked file, byte-equal to the new name
  (`git diff --no-index --stat` empty, 11,197 bytes, the same modification time).
- The return of written files is not in the crates: the client brings back `--fetch` paths and
  cargo products only (`crates/slot/src/client.rs`, `crates/slot/src/artifacts.rs`). It was a
  step of the wrapper that sends every job to the node: after the job, one
  `rsync -a --checksum --update` from the node's tree to the session's, without `.git`, `target`
  and `node_modules`.
- That copy takes every file the node's tree has and the session's lacks. `--update` keeps a
  newer file of the session; it does nothing for a file the session deleted.
- The node's tree held the old name because the wrapper's forward sync drops `--delete` while
  another job of the same tree runs, so that job's written files stay. The session's two probe
  jobs ran in that tree at once (the order of the three jobs is inferred; their sync lines were
  not kept).
- The wrapper was changed the same day: it lists the node's tree before and after the job
  (modification time, size, path) and copies only what is new or changed. Its limits: a file
  written with an old time and the same size does not come back, a name with a newline is not
  handled, and each job pays two listings.

## Recommendation

- The return of a job's written files moves into the client, beside `--fetch` and the cargo
  products, so the rule is tested in this repository and not kept in a script of one machine.
- The set of files is decided on the node, by the job's own record: the node's `slot shell`
  step lists the files of the job's tree whose change time is at or after the job's start (the
  node daemon's clock for both), without `.git`, the target directory and `node_modules`. The
  client copies that list with `--checksum --update`.
- It builds on `story:results-come-back-from-a-node` (the `return` patterns): a pattern narrows
  the list; without one the list is every written file, as the wrapper does today.

## Acceptance

1. With the stand-in node: a job writes `out/report.txt`; after it the session's tree has the
   file, byte-equal.
2. The session deletes a tracked file whose copy the node's tree keeps (a forward sync without
   `--delete`), then runs a job that writes nothing: the file is not in the session's tree after
   the job. This test fails against a return copy of the whole tree.
3. A job rewrites a tracked file: the session's file equals the node's after the job. A file the
   session changed while the job ran is not overwritten.
4. A job that fails (exit code not 0) brings its written files back as a successful one does.
5. A written file under `.git`, the target directory or `node_modules` does not come back.
6. A listing that fails prints one line naming the node and copies nothing.
7. Tests use the stand-in node; no network; each under 60 s.

## Scope

`crates/slot/src/client.rs`, `crates/slot/src/shell.rs`, `crates/slot/src/sync.rs`,
`crates/slot-cli/tests/remote_sync/mod.rs`, `docs/design/remote-pool.md`.
