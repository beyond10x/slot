---
format: aep.planning-md/3
id: story:a-git-dependency-copy-passes-the-linked-cargo-home
kind: story
status: draft
title: A git dependency copy reaches a node whose cargo home is a link into the instance store
relations:
- serves: vision:O2
revision: 1
---
## Outcome

A git dependency the client copies into a node's cargo git database arrives there on a node that
`provision.sh apply` set up, where the cargo home is a link into the instance store.

## Why

Seen 2026-11-26 on both build nodes of one host, release 0.39.1: every copy into the git database
ends with "slot shell: refused: `<home>/.cargo` is a symbolic link" and rsync exit 126, and each
job of a tree with git dependencies prints `slot: git dependencies not copied to <node>`.
`provision.sh` makes `<home>/.cargo` a link to `<store>/cargo`; `shell.rs` `db_copy` refuses a
link at the entry, the database, `git/` or the cargo home. The job then fetches its git
dependencies on the node itself: a public one arrives that way (a consumer's `cargo check`
passed, its fetch 58.7 s); a dependency the node cannot fetch without a credential fails. The
same copy carried `-z` from the host's rsync wrapper until that wrapper stopped compressing
copies into the database.

## Acceptance

- Red first: a test of `db_copy` with the cargo home a link to a directory owned by the build
  user, as `provision.sh` makes it, is refused before the change.
- After it the copy runs when the cargo home is that one link and no other path element of the
  entry, the database or `git/` is a link; a link at any of those three is still refused.
- `provision.sh check` names the cargo home's link target, and a link to anywhere else is drift.
- A job of a tree with a git dependency on a provisioned node prints no `git dependencies not
  copied` line, and the entry is in the node's database after the job.
