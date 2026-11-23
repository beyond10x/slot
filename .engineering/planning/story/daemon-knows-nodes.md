---
format: aep.planning-md/3
id: story:daemon-knows-nodes
kind: story
status: draft
title: The daemon opens its cluster, adds its local node at half the CPUs, and keeps a registry of remote nodes
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
- depends_on: story:host-config-file
- depends_on: story:cluster-types-generated-from-the-spec
- depends_on: story:cluster-conformance-runs-in-the-gate
scope:
- confidence: inferred
  path: crates/slot-cli/tests/handover.rs
- confidence: inferred
  path: crates/slot/src/cli.rs
- confidence: inferred
  path: crates/slot/src/conformance.rs
- confidence: inferred
  path: crates/slot/src/server/handover.rs
- confidence: inferred
  path: crates/slot/src/server/http.rs
- confidence: inferred
  path: crates/slot/src/server/mod.rs
- confidence: inferred
  path: crates/slot/src/server/nodes.rs
- confidence: inferred
  path: crates/slot/src/status.rs
- confidence: inferred
  path: crates/slot/src/types.rs
- confidence: inferred
  path: spec/conformance/implemented.txt
- confidence: inferred
  path: ui/src/generated/types.ts
- confidence: inferred
  path: website/docs/reference/cli.md
revision: 8
---
# Story: the daemon knows its cluster and its nodes

## Outcome

The daemon opens its cluster (topology Hub) at start, adds its local node at the configured share
of the machine (half its CPUs by default), and keeps a registry of remote nodes that the operator
adds, drains, removes and resizes with `slot node …`. The types of the cluster (Node, Placement,
the job's node fields) exist in what the daemon sends, so the web view can be built on them before
placements are made. Every job in the job log names the node it ran on.

## Specification

`spec/domains/cluster.yaml`: Cluster, Node (with `machine`, `location`, `machine_type`, `price_per_hour_cents`, `joined_at`,
`deadline`, `left_at`, `gone_reason`, and the load fields `cpu_busy_milli`, `memory_used_mib`, `disk_free_gib`;
`machine` is accepted by AddRemoteNode and stays empty until
`story:provider-contract-and-node-up-down` launches machines), Placement (type only), Job fields `node_id`, `fell_back`,
`rerun_of`, `sync_ms`; commands OpenCluster (Federation refused), AddLocalNode, AddRemoteNode,
SetNodeCapacity, DrainNode, RemoveNode; invariants of Node (`slots_in_use <= slots`, a deadline on
remote nodes only); views ClusterStatus, NodeList, NodeDetail, Placements, PlacementsOnNode, Jobs,
JobsOnNode.

## Design

`docs/design/remote-pool.md` § 3.3 (a pool per node), § 3.4 (registry in the state directory),
§ 3.6 (registry in the handover state), § 3.7 (the job's node in the log, `fetch_ms`,
`remote_id`), § 7.5 (CLI), the operator's decision of 2026-11-23 (the local node at half the
CPUs).

## Acceptance

1. With no `cluster.toml` on a 20-CPU machine, `slot node list` shows one node, `local`, kind
   Local, Ready, 10 CPUs, 10 slots; `GET /api/nodes` answers the NodeList rows as a bare array
   typed by `crates/slot-cluster-types` (`spec/http.yaml`); the local pool has
   10 tokens; the local node's `cpu_busy_milli`, `memory_used_mib` and `disk_free_gib` follow the
   daemon's `/proc` sampling, refreshed with each state tick.
2. `slot node add <name> --ssh <cmd> --cpus 64 --slots 64 --memory 128 --disk 3500
   --max-worktrees 24 --hours 4 [--location L --machine-type T --price-cents P]` adds a Remote node
   in Joining with a deadline 4 h ahead; a second add of the same name exits 1 with
   `NodeNameTaken`; `--slots 0` exits 1 with `InvalidCapacity`.
3. `slot node drain <name>` moves Ready or Joining to Draining and is refused from Gone with the
   state named; `slot node remove <name>` is refused unless the node is Gone; `slot node
   capacity <name> --slots N` below the slots in use is refused with `CapacityBelowUse`.
4. `GET /api/placements` and `GET /api/nodes/<name>/placements` answer `[]`; every finished job's
   record carries `node` (`local` for every job in this story) and the fields `fell_back`,
   `rerun_of`, `sync_ms`, `fetch_ms`, `remote_id` with their defaults; `slot status` shows the
   node per job; `ui/src/generated/types.ts` is regenerated with them.
5. The registry survives a reload: a handover test with one remote node registered shows the
   same `/api/nodes` before and after. Every new field of a type the daemon sends carries
   `#[serde(default)]`; an older client reads the newer daemon.
6. A hand-written `GET /api/nodes` or `Node` type that reached `main` before this story (one was
   in progress on 2026-11-23, answering `{now_ms, nodes}` with `id`, `memory_bytes`, `region`,
   `instance_type`, `cost_per_hour`, `jobs_done`) is replaced by the generated types and the spec's
   shape; the web UI reads `ui/src/generated/cluster`. A value the hand-written shape had and the
   spec lacks (server time, per-node done and failed counts, the worktree list) is added to the
   spec first, then generated.
7. The scenario ids of OpenCluster, AddLocalNode, AddRemoteNode, SetNodeCapacity, DrainNode,
   RemoveNode and of the views NodeList and NodeDetail are added to
   `spec/conformance/implemented.txt`, and the gate's conformance run passes them.
8. Tests: no network; each under 60 s, the step under 120 s.

## Scope

`crates/slot/src/server/nodes.rs` (new), `crates/slot/src/server/mod.rs`,
`crates/slot/src/server/http.rs`, `crates/slot/src/server/handover.rs`,
`crates/slot/src/types.rs`, `crates/slot/src/cli.rs`, `crates/slot/src/status.rs`,
`crates/slot-cli/tests/handover.rs`, `website/docs/reference/cli.md` (generated),
`ui/src/generated/types.ts` (generated), `spec/conformance/implemented.txt`,
`crates/slot/src/conformance.rs`.

Shared with stories of other epics, none ordered against this one: `server/mod.rs`, `types.rs`,
`cli.rs` and `cli.md` with `story:disk-guard` and `story:admission-waits-for-memory`;
`server/mod.rs` with `story:memory-max-scope-carries-cpu-weight`,
`story:test-execution-bounded-by-pool` and `story:path-shim`; `server/http.rs` with
`story:prometheus-metrics`. Whichever merges second rebases; generated files are regenerated with
`task generate`, never merged by hand.

## Not in this story

Talking to a remote node (`story:node-health-poll`); making placements
(`story:placement-sticks-to-a-node`); the web view (`story:cluster-view-in-web-ui`).
