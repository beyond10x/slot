---
format: aep.planning-md/3
id: epic:compute-cluster
kind: epic
status: draft
title: A compute cluster of build nodes behind the workstation's daemon
summary: Several build nodes (half the workstation, ephemeral cloud machines) behind one endpoint daemon; per-worktree placement; provider contract with account values only in host config.
relations:
- serves: vision:O2
revision: 4
---
# Epic: a compute cluster of build nodes

## Outcome

The daemon every session already calls is the endpoint of a cluster: it keeps the queue, the job
log and the web view, and it runs jobs on several nodes. Half of the workstation's CPUs are the
cluster's local node; one or more ephemeral cloud machines (64 vCPU, 128 GiB, local NVMe) join as
remote nodes and leave at their deadline. A worktree builds on the node that holds its target
directory, and its jobs are not spread across nodes. Sessions change nothing: they keep calling
`slot <command>`.

## Why

O2: one pool decides who builds when, and the decision is data (`AGENTS.md` § Serves). Today the
workstation is the only pool: 20 CPUs for about 24 sessions, load 60 to 110, queue wait p90 about
30 minutes, 50 to 60 GB of free disk against 13 to 25 GB of target directory per worktree
(`docs/design/remote-pool.md` § 1). Remote mode exists but is chosen per command by the caller,
one host per client, with no lifecycle, no shared view and no job log (§ 2.3).

## Specification and design

- ESS, spec first (operator's decision of 2026-11-23): `spec/` — `slot.cluster` (Cluster, Node,
  Placement, Repository opt-in, Job and the node it ran on; the commands and views),
  `slot.hostconfig` (the host's `cluster.toml`, outside every repository), `slot.provisioning`
  (the provider contract). `ess specify validate --path spec` → `slot v1 — 5 file(s), valid`.
  Recorded as `executable-system-specification:compute-cluster`.
- Design: `docs/design/remote-pool.md`. The spec's vocabulary replaces the design's: a "host" is
  a remote node, `slot host …` is `slot node …`.

## Topology

Built: **Hub** — one endpoint daemon, a local node and remote nodes, each node with its own pool
and its own jobserver (§ 3.3). Named and not built: **Federation**, several daemons as peers;
`OpenCluster` refuses it (`slot.cluster.TopologyNotBuilt`). Other shapes stay open: nothing in
the spec ties a node to a cloud.

## Operator's decisions of 2026-11-23

1. Several remote nodes, not one.
2. Spec first in ESS.
3. Preferred topology as above; federation later.
4. Stickiness: a worktree stays on the node that holds its target directory (§ 3.2).
5. Sync without re-sending the tree: rsync deltas, and a new worktree's mirror seeded from a
   sibling copy of the same repository (§ 4.7).
6. Keep the current web UI; add a cluster view with each node's specs, lifetime, jobs and
   placements, and a node filter usable in the existing job views. On 2026-11-23 the operator
   approved an animated cluster start page for it (`docs/design/assets/cluster-start-draft.html`,
   simulated data with generic names); `story:cluster-view-in-web-ui` follows it.
7. The repository is public: nothing about how or where cloud nodes are launched lives in it.
   slot defines the provider contract and ships a generic `aws` provider (over the public `aws`
   CLI) and a `command` provider; every account value (profile, region, instance type, image,
   subnet, security group, connection, hours, cost cap) lives only in the host's
   `$XDG_CONFIG_HOME/slot/cluster.toml`. That configuration is not a story of this repository.

## Stories, in wave order

Derived by `aep plan artifact waves` (the store-wide wave number in brackets; stories of other
epics that share a file sit in the gaps):

1. [W1] `story:cluster-design-and-spec-in-the-gate`, `story:sync-holds-back-secrets`
2. [W2] `story:one-hand-started-node-measured` (phase 0, blocked on the operator's launch)
3. [W3] `story:cluster-types-generated-from-the-spec`
4. [W4] `story:node-boot-script`
5. [W5] `story:cluster-conformance-runs-in-the-gate`
6. [W8] `story:host-config-file`
7. [W12] `story:daemon-knows-nodes`
8. [W13] `story:cluster-view-in-web-ui`
9. [W15] `story:node-health-poll`
10. [W18] `story:placement-sticks-to-a-node`
11. [W19] `story:node-leaves-without-stranding-jobs`
12. [W20] `story:daemon-conforms-to-cluster-spec`
13. [W21] `story:provider-contract-and-node-up-down`
14. [W22] `story:results-come-back-from-a-node`, `story:spot-nodes-and-reclaim`
15. [W23] `story:nodes-start-on-queue-pressure`, `story:sync-sends-the-git-file-list`
16. [W24] `story:new-worktree-seeded-from-a-sibling`, `story:warm-caches-across-node-lives`
17. [W25] `story:cluster-night-measured` (this epic's acceptance night)

Spec first (operator, 2026-11-23 23:4x: "generate a bunch of stuff, conformance, Rust structs,
TypeScript"): items 1, 3 and 5 come before any daemon code. The spec and its HTTP binding
(`spec/http.yaml`) generate the OpenAPI document, the JSON Schemas, the TypeScript types of the UI
and the Rust types of the daemon, including the shape of `cluster.toml`; the committed conformance
suite runs against the daemon in `task check`. Each daemon story adds the scenario ids it
implements to `spec/conformance/implemented.txt`; item 12 closes when the list is complete.

Phases of the design (§ 9): phase 0 is items 1 to 2; phase 1 is items 1, 3, 5 to 12; phase 2 is
items 4, 13 and 14; phase 3 is items 15 to 17.

Files shared with stories of other epics (`crates/slot/src/server/mod.rs`, `cli.rs`, `types.rs`,
`client.rs`, `toolchain.rs`, `server/http.rs`, `Taskfile.yml`, `ui/src`, generated `cli.md` and
`types.ts`) are listed by the collisions of `aep plan artifact waves`; a story names the ones it
knew of when drafted. None is ordered across epics: whichever merges second rebases, and a
generated file is regenerated with `task generate`, never merged by hand.

## Not in this epic

- Federation (named in the spec, refused by `OpenCluster`).
- The concrete binding of an account: values in the host's `cluster.toml`, outside this
  repository.
- Node test runners and browser gates on remote nodes (§ 6: node stays local in every phase).
- From the design's phase 3, not asked for by the operator: out-of-tree path dependencies and
  private git databases (§ 4.4), the `cargo run` split and `--fetch auto` (§ 5.2, § 5.4), mirrors
  at the workstation's path (§ 5.7), moving a worktree by measured load (§ 3.2).
- A reproducibility check of fetched binaries (§ 8.6, § 10 question 7).

## Acceptance

Done when every story's acceptance holds and `story:cluster-night-measured` records one night with two remote nodes, measured from the
endpoint's job log as § 9.5 says, shows: local queue wait p90 under 5 minutes during node hours;
median overhead per remote job under 2 s for warm worktrees; at least half of the rust CPU
seconds run on remote nodes; fallbacks, reruns and lost nodes under 1 % of remote jobs; no
remote test result contradicted by a local run of the same tree; cost within the cap.

## Open questions

- What identifies a repository in the opt-in (`slot node allow <repository>`): root path,
  origin URL or a name (`spec/domains/cluster.yaml`, `UNMAPPED:` on `RepositoryKey`). Blocks the
  opt-in part of `story:placement-sticks-to-a-node`.
- How many reruns one job may have when the rerun's node is lost too (`UNMAPPED:` on
  `Job.first_attempt`). Blocks nothing before `story:node-leaves-without-stranding-jobs`.
- Budget, account and authority (§ 10 question 6): the operator's; phase 0 and the acceptance night
  start paid machines (`decision-blocker:phase-0-node-launch`, `decision-blocker:cluster-night-spend`).
- Two scheduling rules in the approved start-page draft's simulation: a worktree whose node stays
  full for 12 s moves to a node with half its slots free, and a node that joins takes idle
  worktrees from the fullest node. Both contradict the operator's decision of stickiness per
  worktree (decision 4), so neither is in the spec or a story; the asset marks them NOT ADOPTED.
  Blocks nothing.
