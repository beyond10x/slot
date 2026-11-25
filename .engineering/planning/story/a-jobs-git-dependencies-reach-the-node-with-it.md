---
format: aep.planning-md/3
id: story:a-jobs-git-dependencies-reach-the-node-with-it
kind: story
status: draft
title: The git dependencies a job's Cargo.lock names reach the build node with the job
relations:
- serves: vision:O2
revision: 1
---
# The git dependencies a job's Cargo.lock names reach the build node with the job

## Why

On 2026-11-25 every push of one repository's backend failed for a night on the build node: its
gate ran `cargo` inside a task (`task stats`), and cargo could not fetch a git dependency from a
private forge, because the node holds no credential. The fetch-once step of remote mode
(`docs/design/remote-pool.md` § 5.8) runs only for a cargo command the client sends itself; a
wrapper's inner cargo call fetches on the node. The fix that night was by hand: the
workstation's cargo git database was added, clone by clone, to the node's copy list, and it is
gone after a stop and start of the node and stale after a lock file names a new revision.

## Acceptance

- Before a job is placed on a node, the workstation reads every `Cargo.lock` of the job's
  checkout, takes the `git+` sources and their revisions, and makes sure each revision is in the
  workstation's cargo git database (`cargo fetch` there, with the workstation's credentials,
  outside the pool).
- It copies the database entries of those sources to the node's `CARGO_HOME/git/db`, only the
  ones the lock files name, and only sources that pass the remote deny lists. A source on a deny
  list keeps the job on the workstation with the reason `git dependency denied: <source>`.
- A job whose sources are all on the node already copies nothing.
- The node's copy list no longer needs cargo git entries.
- Red first: a test checkout whose `Cargo.lock` names a git source reachable only from the
  workstation builds on a test node that has no network to that source.
