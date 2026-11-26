---
format: aep.planning-md/3
id: story:a-job-that-needs-a-tool-the-node-lacks-runs-here
kind: story
status: draft
title: A job that needs a tool the node lacks runs in the host pool
summary: A repository lists the tools its jobs need in .slot/needs; the client compares it with the node manifest and keeps the job in the host pool when one is missing.
relations:
- serves: vision:O6
- decomposes: epic:compute-cluster
revision: 2
---
## Outcome

A repository declares the tools its jobs need beyond the Rust, Go and Node toolchains, in a file in
the repository. Before a job is placed on a node, slot compares that list with the node's manifest
(`c <tool>` and `tool <name> <version>` lines). A job that needs a tool the node lacks runs in the
host pool, with the reason named in the job's record and on its first output line, in place of
failing on the node.

## Evidence (2026-11-25)

- The node has no `x86_64-w64-mingw32-gcc` and no `musl-gcc` (Amazon Linux 2023 has neither
  package). The connectors gate needs the mingw compiler (`ring` and `libsqlite3-sys` compile C for
  the Windows target) and fails without it; surf asks for musl-tools; substrate says its musl
  target needs no C compiler.
- The wrapper held a job on a difference in the `c` manifest lines until 16:35 CET, when that block
  left `parity()`. Since then no mechanism reads the list of packages the node lacks
  (`contrib/node/README.md`, "held by name"); such a job starts on the node and fails there with
  the tool's own error.
- The route in use until this story is built: a line `origin:<pattern>` in
  `~/.config/slot/remote-deny-origins` keeps a repository's jobs in the host pool. It is meant for
  customer data, so the reason it prints reads "never sync".
- `~/.config/slot/remote-repos` has no reader in the wrapper, the crates or the plugin: a file to
  delete.

## Recommendation

- The file is `.slot/needs` at the top of the repository: one tool per line, `<name>` for a
  command that must exist, `<name> <version>` for one whose `tool` line must carry that version.
  Lines starting with `#` and empty lines are ignored.
- The client reads the file of the job's repository and the node's manifest it already fetches
  for parity, and decides before the sync: every name present on the node (a `c` line that is not
  `missing`, or a `tool` line) and every version equal places the job on the node; otherwise it
  runs in the host pool with `SLOT_LOCAL_REASON` set to `needs <tool>, missing on <node>` (or
  `needs <tool> <version>, <node> has <version>`).
- The decision is in the repository's client, not the host wrapper, so a test in this repository
  covers it. The wrapper's deny list stays for trees that must never reach a node.
- Not in this story: installing the missing tools
  (`story:the-node-has-musl-gcc-and-mingw-by-pinned-source`), and a pinned tool version a tree names
  in a toolchain file (`story:a-job-whose-pinned-tool-a-node-lacks-fails-by-name`).

## Acceptance

- A repository whose `.slot/needs` names `x86_64-w64-mingw32-gcc`, with a node manifest whose `c`
  line for it is `missing`: the job runs in the host pool, and its record has `local_reason`
  `needs x86_64-w64-mingw32-gcc, missing on <node>`.
- The same repository after the node gets the tool (the `c` line is not `missing`): the job runs on
  the node.
- A `.slot/needs` line with a version that differs from the node's `tool` line: host pool, reason
  names both versions.
- A repository with no `.slot/needs`: placement unchanged.
- The node manifest unreadable or older than 5 minutes: the job follows the existing rule for a
  node whose state is unknown, and the reason says the manifest was missing.
- `~/.config/slot/remote-repos` is removed from the host after this story merges (an operator step,
  named in the story's evidence).
