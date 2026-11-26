---
format: aep.planning-md/3
id: story:a-release-task-builds-in-the-pool-and-runs-here
kind: story
status: draft
title: A release task builds in the pool and runs on the workstation
revision: 1
---
# A release task builds in the pool and runs on the workstation

## Why

A repository's release command can be a cargo call that compiles a helper and then uses the
workstation's credentials (a gates policy file, the bot's credentials) to read workflow runs and
push a tag, for example `cargo xtask release-tag <version> <sha>`. On a host whose compiles go to
the build node, the call is sent to the node and fails there (the node has no policy file, by
design). `slot --here` refuses it, because it is a compile. The route that works is three steps
by hand: `slot --fetch target/debug/<bin> cargo build --locked -p <crate>`, then run the binary on
the workstation without slot.

## Acceptance

- One documented command builds a named binary in the pool (on the node when the host sends
  compiles there), copies it back, and runs it on the workstation with the given arguments, its
  exit code the binary's.
- The run on the workstation does not take a slot and gets the caller's environment; the build
  step gets only what a remote build gets.
- A failed build exits non-zero and runs nothing.
- Documented in `website/docs/remote-builds.md` with a release-tag example.
