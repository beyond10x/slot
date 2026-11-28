# slot as a cloudspec architecture, run by weft — design

Status: goal and first trial, 2026-11-28, against slot 0.43.0, cloudspec `0b1cd02` and weft
`5f034cc`. Nothing here is built unless a file and line is cited for it.

Goal: slot is deployable from one cloudspec architecture, with weft as the runtime that realizes
it. cloudspec is the specification level for infrastructure (building blocks, architecture
documents, translation and generation); weft is the runtime that realizes a cloudspec
specification on a placement.

Terms: the **workstation** is the machine whose agent sessions submit jobs; its daemon is the
**local daemon**. A **build node** is a cloud machine with its own **node daemon**. The
**wrapper** is the shell script in front of `slot` on the workstation's agent `PATH` that routes
jobs to nodes.

## 1. The architecture

[`slot.architecture.yaml`](slot.architecture.yaml) (`cloudspec-architecture/1`, written by hand):

- scopes `workstation` and `build-node`;
- workloads: the local daemon (web view on loopback port 7411), the node daemon (reached over
  ssh, port 22), the compile cache server (loopback port 4226), the toolchain refresh (every
  60 s, written as an agent of shape `schedule`, the only scheduled kind the form has);
- volumes: the daemon's state, the node's mirrors;
- actors: an agent session, an operator;
- links: job over a unix socket, view over http, job over ssh, tree sync out and products back
  over rsync, compiles to the cache server.

## 2. The trial

`cloudspec generate kubernetes --from slot.architecture.yaml --out k8s`, the CLI built from
cloudspec `0b1cd02`: exit 0; coverage 45 entries mapped, 15 mapped with loss, 0 refused. The
`valid: true` it prints is the check of its own Kubernetes output: cloudspec `0b1cd02` has no
check of an architecture document. `cloudspec render html` exit 0.

Probes, each a one-field change of the architecture:

| probe | change | result |
|---|---|---|
| t1 | local daemon `kind: process` | exit 1, "only workloads of kind service and agent map in this slice" |
| t2 | `placement` on a scope, `run_as` on a workload | exit 0, no refusal, no coverage entry |
| t3 | a link `protocol: carrier-pigeon` | exit 0 |
| t4 | `shape: daemon` on a service | exit 0, no coverage entry |
| t5 | a link `to:` an undeclared part | exit 0, `valid: true` |

## 3. Gaps in cloudspec

Sent to cloudspec on 2026-11-28 to implement.

Checker defects:

0. No check of an architecture document exists; the generator reads what it maps and ignores
   the rest.
1. A link to an undeclared part passes (t5).
2. Any `protocol` value passes (t3).
3. Unknown fields pass without a refusal or a coverage entry (t2, t4).

Constructs slot needs and the document form lacks:

4. Workload kind process with shape daemon. The blocks have `PackageKind::Process` and
   `ShapeKind::Daemon` (cloudspec `blocks/5-workloads/workloads.yaml:34,49`).
5. No isolation. `IsolationLevel` is `[ProcessSandbox, Container, UserKernel, MicroVm]`
   (cloudspec `blocks/0-protocols/shared.yaml:354`), while the CLI accepts `isolation: none`.
   The local daemon runs jobs as the user, in the user's checkouts, with the user's toolchains.
6. Placement of a scope: a given machine, or a cloud machine with a lease end. Node, NodePool and
   Zone exist in cloudspec `blocks/2-infrastructure/infrastructure.yaml`, not in the form.
7. A host path mounted at its own path: jobs read checkouts at their paths, and a node keeps the
   same paths (same-path mode). The translator knows `HostPath` as a source kind (cloudspec
   `blocks/11-translation/deployments.yaml:185`).
8. A unix socket and a FIFO as ports; a port is a TCP number only.
9. ssh and rsync as link transports with a direction of data; the generator dropped the tree
   sync out and the products back.
10. Reload in place, separate from restart: slot upgrades by SIGHUP and hands running jobs to the
    new binary (`crates/slot/src/server/handover.rs`).
11. A parity constraint between two scopes: the rustc commit, targets, go and node versions match
    before a job runs.
12. A systemd unit generator target.

Kubernetes is no target for slot: actors outside the cluster and loopback ports have no
Kubernetes form (5 links lost). Gaps 4 to 12 are what a weft or systemd target needs.

## 4. Gaps in weft

weft `5f034cc` runs a snippet as the command of an instance's process through its `exec` port
(weft `product/cloud/crates/weft-agent/src/follow.rs:1-25`). It reads no cloudspec architecture
and fetches no artifact. Needed, in order:

- A. Build nodes as a weft node pool: provision the machine, run
  [`contrib/node/provision.sh`](../../../contrib/node/README.md) `apply` with the node's config, ssh
  for the build key, a lease end, a drain. This replaces the hand-run setup and the daemon's node
  registry.
- B. The node daemon as a weft workload on those nodes: a process package (the release tarball by
  digest), shape daemon, the instance store mounted, readiness `slot status`.
- C. The local daemon, once cloudspec gaps 5, 7, 8 and 10 are closed; until then it stays a
  systemd user service (`contrib/slot.service`).

## 5. Gaps in slot

What a deployment can install is what the repository ships. Today the workstation side of node
mode is partly outside it:

- S1. The wrapper and its helpers are not in the repository: the `slot` wrapper on the agent
  `PATH` (node route, hold files, toolchain parity wait, refusals of compiles under the cache
  directory and of `--here` for compiles, return sync switch), the `--manifest-path` tree change,
  the list of variables sent to a node, the refusal log and an `rsync` wrapper.
  `contrib/slot-shim` (the `cargo`/`go` shim) and `contrib/node/` are in it.
- S1a. The `rsync` wrapper changes what the sync sends: it replaces the client's
  `--filter=:- .gitignore` (`crates/slot/src/sync.rs:644`) with an exclude list of cargo build
  directories only (`<dir>/target/` beside each `Cargo.toml`, and any `target` directory holding
  `CACHEDIR.TAG` or `.rustc_info.json`), so other ignored files (`node_modules`, generated files)
  reach the node; it adds a bandwidth cap and three upload slots; it drops `--delete` while
  another job of the same tree runs. The repository's sync and the deployed sync differ.
- S2. The node registry (`cluster.toml`) is written by hand; with gap A it comes from weft's node
  pool.
- S3. The architecture in this directory is checked by nothing; once cloudspec has a check of an
  architecture document, `task check` runs it.
- S4. slot has an ESS system of its cluster (`spec/`, `slot.cluster`, `slot.hostconfig`,
  `slot.provisioning`, `slot.pool`). cloudspec lists an ESS system as an inbound source; whether
  `spec/` translates into the architecture of this directory is not tried.
