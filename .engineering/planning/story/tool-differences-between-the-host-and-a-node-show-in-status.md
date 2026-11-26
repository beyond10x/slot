---
format: aep.planning-md/3
id: story:tool-differences-between-the-host-and-a-node-show-in-status
kind: story
status: draft
title: A difference in tsc, prettier or esbuild between the host and a node shows in slot status within a minute, and holds no job
summary: The manifest comparison reports tool lines; slot status shows a drift line
relations:
- serves: vision:O6
- decomposes: epic:compute-cluster
- informed_by: story:node-installs-its-toolchains-by-script
revision: 2
---
## Outcome

A difference between the workstation's and a build node's TypeScript and JavaScript tools (`tsc`,
`prettier`, `esbuild`) is seen within the manifest refresh period, without anyone running
`push.sh check`. `slot status` shows one drift line naming each tool and both versions. No job is
held for it.

## Evidence (2026-11-25)

- `toolchains.sh` prints a `tool <name> <version>` line for each of the three tools, and
  `push.sh compare` reports a difference both ways (commit 5451f26 of branch `bare-node`).
- The wrapper's `parity()` reads the `c`, `rust <active toolchain>`, `go` and `node` lines only. A
  `tool` line holds no job.
- `push.sh check` runs when somebody runs it. A node that is provisioned and then drifts (a tool
  upgraded on the workstation only) is not noticed until a job fails in a test lane, for example
  the one `esbuild` test of ess that needs the tool on `PATH`.
- The refresh (`toolchains-refresh.sh`, every 60 s) already writes `toolchains-local.txt` and
  `toolchains-node.txt`; both hold the `tool` lines once both machines run the new `toolchains.sh`.

## Decision (host operator, 2026-11-25)

The comparison lives in the daemon, not in the host's refresh script: it is testable in this
repository and needs no host file.

## Recommendation

- The daemon reads the two manifest files (`toolchains-local.txt`, `toolchains-node.txt`) on its
  census tick, compares their `tool` lines and keeps one drift line per difference: `<tool> here
  <version|missing>, node <version|missing>`. Nothing is written to disk; an unreadable or empty
  node manifest gives no tool line, and a node manifest older than 5 minutes gives none either.
- `slot status` and `/api/state` show the lines under the node (a field with `#[serde(default)]`),
  as `drift: tool tsc here 6.0.3, node 6.0.2`.
- No hold and no parity wait: the line is information. A tool a tree pins by name stays the
  business of `story:a-job-whose-pinned-tool-a-node-lacks-fails-by-name`.
- Which tools count is the set of `tool` lines the manifests carry; a new tool in `toolchains.sh`
  needs no change in the daemon.

## Acceptance

- Workstation `tsc` 6.0.3, node `tsc` 6.0.2: within 2 refresh periods `slot status` shows
  `drift: tool tsc here 6.0.3, node 6.0.2`, and a job on the node still starts at once.
- Both equal: no drift line.
- A node manifest older than 5 minutes: no tool drift line; a tool missing on the node shows
  `node missing`.
- A daemon test with two manifest files in a temporary directory covers each case.
