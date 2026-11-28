# slot as a cloudspec architecture, run by weft — design

Status: goal and first trial, 2026-11-28, against slot 0.43.0, cloudspec `0b1cd02` and weft
`5f034cc`. Nothing here is built unless a file and line is cited for it.

Goal: slot is deployable from one cloudspec architecture, with weft as the runtime that realizes
it. cloudspec is the specification level for infrastructure (building blocks, architecture
documents, translation and generation); weft is the runtime that realizes a cloudspec
specification on a placement.

Terms: the **workstation** is the machine whose agent sessions submit jobs; its daemon is the
**local daemon**. A **build node** is a cloud machine with its own **node daemon**. The
**wrapper** is the shell script in front of `slot` on the workstation's agent `PATH`
(`contrib/slot-wrapper`) that routes jobs to nodes.

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

State on cloudspec `9aa52c3`: gaps 0 to 11 below are closed there. `slot.architecture.yaml` now
uses those constructs: the three daemons are `kind: process` with `run_as: {user: deployer}`,
`isolation: none` and `upgrade: {reload: {signal: HUP}}`; scopes carry `placement`; the job
socket and the jobserver are `unix` and `fifo` ports; sync and return are `protocol: ssh` links
with `data: push` and `data: pull`; `parity` names the facts that must match. `task
architecture` runs `cloudspec check --json` on it: exit 0, no violation. Gap 12 (the systemd
generator, with ports the process binds itself and paths under the deployer's home and runtime
directory) is in progress there; the socket, FIFO and home paths in the file are placeholders
until it lands.

State on cloudspec `f573374`: slot's API is a contract of cloudspec's `ci` pack
(`contracts/ci/job_runner.yaml`, `cloudspec-contract/1`), not one of slot's own: a hosted CI
service or a runner farm plays the same contract. The local daemon `provides` `job_runner` on its
`jobs` port with the guarantees `exit_code: passed_unchanged`, `did_not_run_code: reserved`,
`run_limit: named_maximum`, `queue_order: priority`, `placement: remote_only_when_asked` and
`toolchain_parity: checked_before_remote`; the node daemon provides it with `placement: local`;
the compile cache server provides `compile_cache`, which the local daemon needs. slot's ESS
domain `slot.pool` (`spec/domains/pool.yaml`) is the binding's side of `job_runner`. The
systemd units generated from this file do not yet equal `contrib/slot.service` and the node's
unit; the differences went to cloudspec as eight generator gaps (`${home}` in a command, claims
as cgroup ceilings, the stop and kill mode, a system unit with a build user, a template unit per
node, a pass-through in the binding) and are in progress there.

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

What a deployment can install is what the repository ships. The workstation side of node mode
was partly outside it; S1 and S1a are closed:

- S1, closed. The `slot` wrapper on the agent `PATH` is `contrib/slot-wrapper`, its helpers
  `contrib/tree-cd.sh` (the `--manifest-path` tree change), `contrib/env-names.sh` (the variables
  sent to a node) and `contrib/refusal-log.sh` (the refusal log); `task node-wrapper` installs
  them. `contrib/slot-wrapper.test.sh` (in `task check`) covers the node route, hold files, the
  toolchain parity wait with its bound, the refusals (a compile under the cache directory,
  `--here` for a compile, a script under the session scratch directory), `--here`, `--local`, the
  return sync switch, the tree change, the forwarded variables and the refusal log. The wrapper
  has no default node: the route must name one. `contrib/slot-shim` (the `cargo`/`go` shim) and
  `contrib/node/` were in it already. Every release archive carries these files under
  `contrib/` (`.github/workflows/release.yml`, test
  `every_release_archive_carries_the_workstation_and_node_files`).
- S1a, closed. The `rsync` wrapper's filters are the client's (`crates/slot/src/sync.rs`): the
  sync always leaves out cargo build directories (`<dir>/target` beside each `Cargo.toml`, and
  any `target` directory holding `CACHEDIR.TAG` or `.rustc_info.json`); it applies every
  `.gitignore` unless `SLOT_SYNC_IGNORED=1`, which `contrib/slot-wrapper` sets so a node gets
  what git ignores too; while another job of the same tree runs on the workstation it deletes
  only the paths its previous sync sent that are gone here (a registry of running jobs in the
  runtime directory, a list of sent paths per mirror); an upload holds one of `SLOT_UPLOAD_SLOTS` (3) slots
  and runs under `SLOT_UPLOAD_BWLIMIT` (2000 KiB/s, compressed). No `rsync` wrapper is needed on
  `PATH`; the repository's sync is the deployed sync.
- S2. The node registry (`cluster.toml`) is written by hand; with gap A it comes from weft's node
  pool.
- S3. The architecture in this directory is checked by nothing in slot's gate. cloudspec has
  `cloudspec check --json <arch>` from cloudspec `8a3e9b1`; against it the file is refused for
  four link protocols (`unix`, `ssh`, `rsync` twice) until cloudspec's data-link and socket-port
  constructs land.
- S4. slot has an ESS system of its cluster (`spec/`, `slot.cluster`, `slot.hostconfig`,
  `slot.provisioning`, `slot.pool`). cloudspec lists an ESS system as an inbound source; whether
  `spec/` translates into the architecture of this directory is not tried.
