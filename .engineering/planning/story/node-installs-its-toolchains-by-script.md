---
format: aep.planning-md/3
id: story:node-installs-its-toolchains-by-script
kind: story
status: implemented
title: A build node starts bare and provision.sh installs everything; nothing is copied to it
summary: provision.sh installs rust toolchains, cargo tools and home tool copies from pinned archives; push.sh copies /usr and /etc only
relations:
- serves: vision:O6
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-26T12:18:58Z", actor: "human:operator", revision: 4, decided_on: {"recorded":{"health_observation":3}}}
- {from: "proposed", to: "active", at: "2026-11-26T12:18:59Z", actor: "human:operator", revision: 5, decided_on: {"recorded":{"health_observation":3}}}
- {from: "active", to: "implemented", at: "2026-11-26T12:19:06Z", actor: "human:operator", revision: 6, decided_on: {"recorded":{"test_result":1,"health_observation":3}}}
---
## Outcome

A build node starts as a bare machine image and `provision.sh apply` makes it complete: system
packages, Rust toolchains at exact versions, cargo tools, language runtimes, browsers and the
home-directory tool trees, all from package repositories and release archives, each pinned by
version or SHA-256 in the node's config. Nothing is copied from the workstation to a node ahead of
jobs. A job's own tree is still synced when the job runs.

## Context

On 2026-11-25 the first `push.sh apply` to a new node copied about 65 GB at about 15 MB/s
(`/usr` 38 GB, Rust toolchains 13 GB, cargo tools 2.4 GB, `HOME_COPIES` about 10 GB) and was
stopped at 29 GB on the owner's order: a node holds no state, so a node needs no copy. Today jobs
on a node run against the workstation's `/usr` (`BindReadOnlyPaths=$STORE/rootfs/usr:/usr`) and
the parity check compares the workstation's toolchain lines and the digest of `/usr` and `/etc`
with the node's.

## Design

1. The node's `/usr` is its own. The units stop binding `rootfs/usr` and `/etc` entries from a
   copy; `ETC_EXTRA` and `HOME_COPIES` become package and archive installs listed in the node's
   config (`PACKAGES`, `RUST_TOOLCHAINS`, `TOOL_PINS`, `HOME_TOOLS`).
2. Parity compares versions, not bytes: the toolchain lines of `toolchains.sh` (`rust` name and
   commit, `go`, `node`, the C compilers' `--version`) must equal the workstation's for the
   toolchain a job's tree resolves to; a job whose toolchain differs is held with the line named,
   as today.
3. `push.sh` stops copying. Its `check` stays as the parity read; its `apply` is removed.
4. A change that widens what the node accepts gets a read-only review before the root script
   reaches a node.

## Acceptance

- `provision.sh apply` on a bare Amazon Linux 2023 instance leaves `provision.sh check` and the
  parity check at `0 drift`, with no `push.sh apply` run; `test.sh` covers each install step.
- A first build of a repository on that node exits 0.
- `push.sh apply` is gone from the README and the docs.

## Not in this story

- A snapshot or image of a finished node.
- Per-repository mirrors (`story:builds-spread-over-two-nodes`).
