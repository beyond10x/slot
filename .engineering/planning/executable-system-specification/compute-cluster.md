---
format: aep.planning-md/3
id: executable-system-specification:compute-cluster
kind: executable-system-specification
status: draft
title: ESS specification of the compute cluster
summary: 'spec/: slot.cluster, slot.hostconfig, slot.provisioning; validates on ess 0.98.0.'
relations:
- specifies: epic:compute-cluster
revision: 2
---
# Executable system specification: the compute cluster

The ESS specification of the compute cluster, written before any of it is built (operator's
decision of 2026-11-23, `AGENTS.md` § ESS).

## Files

`spec/ess-inputs.yaml` lists them:

- `spec/system.yaml` — system `slot`, format `ess/37`; every scenario first opens the cluster
  `main` with topology `Hub`.
- `spec/components.yaml` — `slot-daemon` (owns `slot.cluster`, `slot.hostconfig`; reached over
  the network API) and `node-provider` (owns `slot.provisioning`; reached as a program).
- `spec/domains/cluster.yaml` — Cluster (topology Hub or Federation), Node (kind Local or Remote;
  cpus, slots, memory, disk, max worktrees; slots in use; lease deadline; Joining, Ready,
  Draining, Gone), Placement of a worktree on a remote node, Repository opt-in, Job and the node
  it ran on. Commands OpenCluster, AddLocalNode, AddRemoteNode, AdmitNode, RecordNodeState,
  SetNodeCapacity, DrainNode, MarkNodeGone, RemoveNode, AllowRepository, DisallowRepository,
  PlaceWorktree, UnplaceWorktree, SubmitJob, StartJob, EndJob, WithdrawJob, FallBackJob. Views
  ClusterStatus, NodeList, NodeDetail, Placements, PlacementsOnNode, Repositories, Jobs,
  JobsOnNode.
- `spec/domains/hostconfig.yaml` — the host's `cluster.toml`: HostConfig (`[node.local]` share),
  Provider (`[provider.<name>]`, kind `aws` or `command`), NodeTemplate (`[template.<name>]`);
  the rules `slot cluster config check` applies; views that publish no account value.
- `spec/domains/provisioning.yaml` — the provider contract: Machine, ProvisionMachine
  (idempotent by request key, self-termination armed before the answer), TerminateMachine
  (idempotent), the Machines view (`describe`).

- `spec/domains/hostconfig.yaml` also declares the whole file as `slot.hostconfig.ClusterFile`
  (closed structs; providers and templates as maps by name), the type the daemon parses into.
- `spec/http.yaml` — `ess-http/1` binding of the `slot-daemon` component: 11 views and 6 operator
  commands under `/api/` (`GET /api/nodes` answers NodeList as a bare array); 17 routes, 0 open.

## Projections (verified with ess 0.98.0, written to a scratch directory, not committed)

- `ess generate generate --kind openapi --http spec/http.yaml`: `slot-daemon.yaml`,
  `node-provider.yaml`.
- `ess generate generate --kind schema`: 129 JSON Schemas.
- `ess generate types --target typescript|rust --root <view>.row … --root
  slot.hostconfig.ClusterFile --http spec/http.yaml`: 37 model types each. The Rust crate enables
  serde_json `arbitrary_precision` by default; slot depends on it with `default-features = false`.
- `ess verify conform synthesize --path spec`: 109 scenarios, 46 refusals.

Planned as `story:cluster-types-generated-from-the-spec` and
`story:cluster-conformance-runs-in-the-gate`.

## Rules the specification states

- A worktree has at most one placement (`PlaceWorktree/already-placed`); the local node takes
  none (`local-node`); a node that is not Ready takes no new placement or job (`not-ready`), so a
  draining node gets none; a lease shorter than 30 minutes takes no placement
  (`lease-too-short`).
- A job of a placed worktree is queued only on its placement's node
  (`SubmitJob/placed-elsewhere`); the only move between nodes is `FallBackJob` to the local node.
- Slots in use never exceed capacity: Node invariant `slots_in_use <= slots`;
  `RecordNodeState/exceeds-capacity`; `SetNodeCapacity/below-use`;
  `SubmitJob/too-many-tokens`.
- A node that drains or leaves ends its placements (`DrainNode`, `MarkNodeGone`, `affects:`).
- A remote node has a deadline, the local node none (Node invariants).

## Results

`ess specify validate --path spec` (ess 0.98.0): `slot v1 — 5 file(s), valid`.

`ess verify conform synthesize --path spec`: 109 scenarios, 46 refusals (19 `ESS-SYNTH-001`,
6 `ESS-SYNTH-003`, 21 `ESS-SYNTH-004`). Most refusals follow from synthesis arranging only one
level of rows (a Job needs a Node, a Placement a Node and a Repository);
`story:daemon-conforms-to-cluster-spec` reads them and covers the gaps with authored scenarios.

## Open markers

- `UNMAPPED:` on `slot.cluster.RepositoryKey`: what identifies a repository in the opt-in.
- `UNMAPPED:` on `slot.cluster.Job`: how many reruns one job may have.
- `LIMIT:` on `slot.cluster.PlaceWorktree`: the `max_worktrees` bound is not expressible as a
  row-set count against a field of the named node.
