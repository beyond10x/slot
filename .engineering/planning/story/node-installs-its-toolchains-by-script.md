---
format: aep.planning-md/3
id: story:node-installs-its-toolchains-by-script
kind: story
status: draft
title: The node installs its toolchains and tools by script; the push copies only /usr and /etc
summary: provision.sh installs rust toolchains, cargo tools and home tool copies from pinned archives; push.sh copies /usr and /etc only
relations:
- serves: vision:O6
revision: 1
---
## Outcome

`provision.sh apply` installs the node's Rust toolchains, cargo tools and home-directory tool
copies from their release archives, each checked against a pinned SHA-256, so `push.sh apply`
copies only what no release archive can give: the workstation's `/usr` and the `/etc` entries
jobs read. A new node then needs the `/usr` copy and no more.

## Context

On 2026-11-25 the first `push.sh apply` to a new node copied about 65 GB at about 15 MB/s: `/usr`
38 GB, the Rust toolchains 13 GB, the cargo tools 2.4 GB, the `HOME_COPIES` directories about 10
GB. `/usr` stays a copy: jobs run against the workstation's own libraries and compilers
(`BindReadOnlyPaths=$STORE/rootfs/usr:/usr` in the units) and the check compares its digest. The
rest has a release archive: `rustup toolchain install <version>`, the pinned tools of `TOOL_PINS`,
the Go modules from the module proxy.

## Acceptance

- `provision.sh apply` installs each toolchain named in a new `RUST_TOOLCHAINS` list at the exact
  version, and `push.sh apply` skips `~/.rustup` and `~/.cargo/bin` entries that the node has.
- `push.sh check` compares the toolchain manifest as before and prints `0 drift` on a node built
  this way; the digests of the installed toolchains equal the workstation's.
- `test.sh` covers a toolchain version that is missing, wrong, and right.

## Not in this story

- The workstation's `/usr` and `/etc` entries.
- A snapshot or image of a finished node.
