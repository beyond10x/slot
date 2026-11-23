---
format: aep.planning-md/3
id: story:cluster-types-generated-from-the-spec
kind: story
status: draft
title: The cluster API, its types and the cluster file shape are generated from the spec
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
- depends_on: story:cluster-design-and-spec-in-the-gate
scope:
- confidence: inferred
  path: Cargo.lock
- confidence: inferred
  path: Cargo.toml
- confidence: inferred
  path: Taskfile.yml
- confidence: inferred
  path: crates/slot-cluster-types
- confidence: inferred
  path: crates/slot-docs
- confidence: inferred
  path: crates/slot/Cargo.toml
- confidence: inferred
  path: spec/generated
- confidence: inferred
  path: spec/http.yaml
- confidence: inferred
  path: ui/src/generated/cluster
revision: 2
---
# Story: the cluster API, its types and the cluster file shape are generated from the spec

## Outcome

The cluster specification is the source of the cluster's API and types. `task generate` projects
from `spec/` an OpenAPI document of the daemon's cluster routes, the JSON Schema of every
cluster type and of `cluster.toml`, the TypeScript types the web UI reads and a Rust data crate
the daemon uses; `task check` fails when any of them differs from what the spec generates. No
daemon behaviour changes in this story.

## Specification

All of `spec/`, plus the HTTP binding `spec/http.yaml` (`ess-http/1`, 17 routes, 0 open): the
views ClusterStatus, NodeList (`GET /api/nodes`, a bare array), NodeDetail, Placements,
PlacementsOnNode, Repositories, Jobs (`GET /api/cluster/jobs`), JobsOnNode, HostConfigs,
Providers, NodeTemplates, and the operator commands AddRemoteNode, SetNodeCapacity, DrainNode,
RemoveNode, AllowRepository, DisallowRepository. The shape of `cluster.toml` is the type
`slot.hostconfig.ClusterFile` (closed structs; `provider` and `template` are maps by name).

## Design

How the spec's types and today's ts-rs types fit: the spec owns every cluster type (rows of the
views above and `ClusterFile`); ts-rs keeps every other type of the API (`State`, `Tick`, `Job`,
the census) as today. One name has one source: `crates/slot-docs` fails when ts-rs emits a type
whose name a spec projection also emits. The generated Rust crate is used by the daemon as it is,
never edited; a field the daemon needs that the spec lacks is added to the spec first.

Projections, each checked byte for byte by its own `--check` (verified with ess 0.98.0 in the
planning worktree, written to a scratch directory):

- `ess generate generate --kind openapi --http spec/http.yaml --out spec/generated/openapi`
  (`slot-daemon.yaml`, `node-provider.yaml`);
- `ess generate generate --kind schema --out spec/generated/schema` (129 schemas);
- `ess generate types --target typescript --http spec/http.yaml --root <view>.row … --root
  slot.hostconfig.ClusterFile --out ui/src/generated/cluster` (37 model types);
- the same with `--target rust --package slot-cluster-types --out crates/slot-cluster-types`.

## Acceptance

1. `spec/http.yaml` validates (`ess specify http validate --path spec --binding spec/http.yaml`
   prints `17 route(s), 0 open, valid` or more routes, never an open one).
2. `task generate` writes the four projections above; a second run changes nothing; `task check`
   runs each projection's `--check` and exits non-zero when one file differs (shown once in the
   pull request by editing a field summary in the spec without regenerating).
3. `crates/slot-cluster-types` is a workspace member that `crates/slot` depends on with
   `default-features = false`, so serde_json's `arbitrary_precision` does not reach the rest of
   the workspace: `cargo tree -e features -i serde_json` lists no `arbitrary_precision`. A test
   reads `{"node_id":"local","cluster_id":"main","kind":"Local","state":"Ready","cpus":10,
   "slots":10,"slots_in_use":0,"memory_gib":32,"disk_gib":1800,"running_jobs":0,"queued_jobs":0,
   "slot_version":"x"}` into the NodeList row type and writes it back byte-equal.
4. A test parses a name-free `cluster.toml` fixture (one `command` provider, one `aws` provider with
   placeholder values, one template, `[node.local]`) into `ClusterFile`, and a file with an
   undeclared key (`secret = "x"` in a provider table) is refused naming the key.
5. `crates/slot-docs` fails when ts-rs and the spec's TypeScript both define one type name, shown
   by a test.
6. `ui/src/generated/cluster/types.ts` type-checks in the UI build; no hand-written cluster type
   remains in `ui/src`.
7. `task public-safety` passes on everything generated. Tests: no network; each under 60 s, the
   step under 120 s.

## Scope

`spec/http.yaml`, `spec/generated/`, `crates/slot-cluster-types/` (generated), `Cargo.toml`,
`Cargo.lock`, `crates/slot/Cargo.toml`, `crates/slot-docs/`, `ui/src/generated/cluster/`
(generated), `Taskfile.yml`.

`Taskfile.yml` is also edited by `story:cluster-design-and-spec-in-the-gate`, which this story
depends on.

## Not in this story

The daemon answering `GET /api/nodes` from the generated types (`story:daemon-knows-nodes`); the
conformance run (`story:cluster-conformance-runs-in-the-gate`).
