---
format: aep.planning-md/3
id: story:a-git-dependency-copy-resolves-a-linked-cargo-home
kind: story
status: draft
title: A job's git dependencies reach the node also when the cargo home is a symbolic link
relations:
- serves: vision:O6
revision: 2
---
## Outcome

A job whose `Cargo.lock` has git dependencies gets them on the node also when the client's
`~/.cargo` is a symbolic link: the client resolves the link before it asks the node to copy.

## Why

Seen 2026-11-27 on a remote job: `slot shell: refused: <home>/.cargo is a symbolic link`, then
`slot: git dependencies not copied to <node>: <name>: rsync exited with exit status: 126`. The
build passed only because the node already held the revision. The node's `slot shell` refuses a
path with a link in it, and the client sends the cargo home path without resolving it.

## Acceptance

- Red first: a test with a cargo home that is a link: the copy step's path has no link in it.
- A path that still holds a link after resolution is refused by the client with the path named,
  before ssh.
