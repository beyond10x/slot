---
format: aep.planning-md/3
id: story:a-node-has-firefox
kind: story
status: draft
title: A node has Firefox for browser tests
relations:
- serves: vision:O6
revision: 1
---
## Outcome

A node has Firefox, installed by `contrib/node/provision.sh` from Mozilla's release archive at a
pinned version and SHA-256, on every job's PATH, so browser tests that start Firefox pass on a
node as on the workstation.

## Why

On 2026-11-26 two browser test targets of a consumer failed on a node with `this runner did not
start Firefox for BiDi ... spawn (No such file or directory)`. The node had no Firefox.

## Acceptance

- Red first: on a node `firefox --version` in a job fails before the change.
- The archive's SHA-256 is the one in Mozilla's `SHA256SUMS` of that release; a wrong checksum
  is refused; a node whose config does not ask for Firefox installs none.
- The libraries Firefox needs and the image lacks are in the node's package list; a headless
  start in a job prints its WebDriver BiDi address.
- `toolchains.sh` prints one `tool firefox <version>` line, equal on workstation and node.
- `check --canary`, or a command the README names, proves the headless start on a node.
- The consumer's two test targets pass on the node.
