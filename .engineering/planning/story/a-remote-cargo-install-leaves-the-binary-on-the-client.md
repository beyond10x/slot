---
format: aep.planning-md/3
id: story:a-remote-cargo-install-leaves-the-binary-on-the-client
kind: story
status: draft
title: A remote cargo install leaves the binary on the machine that asked for it
relations:
- serves: vision:O2
revision: 1
---
## Outcome

`cargo install` through slot compiles on a build node and leaves the installed binary on the
machine that asked for it, at the path cargo would have written there.

## Why

A `cargo install` sent to a node installs into the node's cargo home: the workstation gets no
binary, and the node gets one that no later job asked for. Sessions get round it with `slot
--fetch <path>` and a `--root` of their own, or with `--here`, which compiles on the
workstation. Seen on 2026-11-26; not measured separately from other `--here` jobs.

## Acceptance

- Red first: a remote `cargo install --path <crate>` that exits 0 leaves no binary in the
  client's install root.
- After the change the binaries cargo installed are in the client's install root (`--root`,
  `CARGO_INSTALL_ROOT`, else the cargo home's `bin`) with their mode; the node's cargo home has
  no new binary and its `.crates.toml` is unchanged.
- `cargo install --list`, `--version` and a failed install bring nothing back.
- `SLOT_NO_PRODUCTS=1` turns it off, as for cargo's other products.
- The docs page on remote builds names it.
