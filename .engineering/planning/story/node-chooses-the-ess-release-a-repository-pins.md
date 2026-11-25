---
format: aep.planning-md/3
id: story:node-chooses-the-ess-release-a-repository-pins
kind: story
status: draft
title: The node chooses the ess release a repository pins, as the workstation does
revision: 1
---
# The node chooses the ess release a repository pins, as the workstation does

## Why

On 2026-11-25 the build node carried one pinned `ess`, 0.99.1, installed by `provision.sh`
(`TOOL_PINS`), while the workstation's base `ess` was 0.98.0. A repository whose
`ess-inputs.yaml` has the line pin `requires: ess 0.98` and runs `--strict-requires` was refused on
the node (`requires ess 0.98 and this is ess 0.99.1, which is newer`) and passed on the
workstation.

`ess` chooses the release itself (`crates/edge/ess-cli/src/toolchain.rs` of the ess repository):
an exact `requires: ess X.Y.Z` runs that release from `~/.cache/ess/toolchains/X.Y.Z`, downloaded
from the ess release and checked against its `SHA256SUMS`; a line pin or none runs the base
binary. One base on both machines gives one choice per repository.

## Acceptance

- `provision.sh pins` lists no `ess`; `push.sh bins` lists `ess`; `push.sh check` prints
  `sum ess <sha256>` on both sides, and a node `ess` of another digest is drift.
- `contrib/node/test.sh`, where an `ess` is on PATH: base `ess` with an exact pin `0.99.1` runs
  0.99.1 from the cache; a line pin the base cannot serve fails with ess's own line and runs no
  other release; the base's own line runs the base.
- The node's `/srv/slot/cargo/bin/ess` equals the workstation's `~/.cargo/bin/ess` after the
  bundle and a `push.sh apply` (`push.sh check`: 0 drift).
- Not before a repository that needs a release above the workstation's base pins it exactly.
