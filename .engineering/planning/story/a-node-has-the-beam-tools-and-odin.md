---
format: aep.planning-md/3
id: story:a-node-has-the-beam-tools-and-odin
kind: story
status: draft
title: A node has the BEAM tools and Odin
revision: 3
---
## Outcome

A node has Erlang/OTP, Elixir, Gleam and Odin in the versions a consumer's jobs use on the
workstation, installed by `contrib/node/provision.sh` from pinned archives, and the consumer's
documentation and reader checks pass on the node.

## Why

A consumer's `task docs-generate` failed on a node with `gleam: command not found`. Read on
the node: no `gleam`, `elixir`, `mix`, `erl` or `odin` on a job's PATH. On the workstation the
consumer's jobs put these on PATH themselves from directories under the home that the node
does not have:

- Elixir 1.20.4 (needs `erl`; the workstation has Erlang/OTP 29, erts 17.0.6, from the
  distribution's package)
- Gleam 1.19.0
- Odin nightly 2026-09-01 (release `dev-2026-09`)

The consumer's CI pins Elixir 1.20.4 and Gleam 1.19.0. The consumer has no checksums for the
three directories.

## Acceptance

- Red first: on a node the canary commands the consumer named fail before the change:
  its documentation generation and check tasks and the tests of its C reader.
- Each tool is a pin with a SHA-256 of its archive from the upstream release. The consumer's
  tasks call `gleam`, `elixir`, `mix`, `odin` and `erl` by name, as its CI provides them, so
  each is on every job's PATH on the node; the install path is the node's choice (the consumer
  agreed).
- Erlang/OTP on a node: a prebuilt archive that loads with the node's C library, or a source
  build as the build user; the build's time is measured and named, and it runs once, not per
  apply.
- `toolchains.sh` prints one line per tool; the workstation's and the node's lines are equal.
- The three canary commands pass on the node; then the consumer's line can leave the list of
  trees that never go to a node (the list is the operator's).
- A node without the config key that asks for these tools installs none of them.

## Notes

- Maven runs offline against the local repository in the consumer's tasks; whether the node
  has the artifacts those tasks need is part of the red check.
- The C reader's tests need clang and libubsan; the node has `libubsan` 14.2.1 and gcc 11.5.0.
  Whether that pair links is part of the red check.
