---
format: aep.planning-md/3
id: story:provider-contract-and-node-up-down
kind: story
status: draft
title: slot node up and down launch and end nodes through the provider contract, with kinds aws and command
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
- depends_on: story:host-config-file
- depends_on: story:node-health-poll
- depends_on: story:node-leaves-without-stranding-jobs
- depends_on: story:daemon-conforms-to-cluster-spec
- depends_on: story:cluster-design-and-spec-in-the-gate
scope:
- confidence: inferred
  path: contrib/slot-provider-double
- confidence: inferred
  path: crates/slot/src/cli.rs
- confidence: inferred
  path: crates/slot/src/provider
- confidence: inferred
  path: crates/slot/src/provider.rs
- confidence: inferred
  path: crates/slot/src/server/http.rs
- confidence: inferred
  path: crates/slot/src/server/nodes.rs
- confidence: inferred
  path: website/docs/cluster/providers.md
- confidence: inferred
  path: website/docs/reference/cli.md
revision: 4
---
# Story: the provider contract, and `slot node up` and `slot node down`

## Outcome

`slot node up <template>` launches a machine through the template's provider, registers it as a
remote node with its deadline, and the machine ends itself at that deadline whatever happens to
the workstation. `slot node down <name>` drains and terminates it. slot ships two provider kinds:
`aws`, a generic provider over the public `aws` CLI whose every value comes from the host
configuration, and `command`, an executable the host configuration names that speaks the
contract. A name-free test double of a `command` provider (POSIX sh) is in the repository.

## Specification

`spec/domains/provisioning.yaml`: Machine, ProvisionMachine (idempotent by request key;
InvalidRequest, ProviderNotConfigured, LifetimeOverProviderMax, KeyTerminated, CostCapReached,
NoCapacity, SelfTerminationNotArmed), TerminateMachine (idempotent), view Machines (`describe`);
`spec/domains/hostconfig.yaml` Provider and NodeTemplate; `spec/domains/cluster.yaml`
AddRemoteNode (`machine`, `deadline`, `location`, `machine_type`, price).

## Design

`docs/design/remote-pool.md` § 7.5 (provider program, its wire), § 7.4 (join, drain, stop),
§ 8.2 (a key pair per node, deleted at down), § 8.5 (orphan listing), § 8.7 (four layers; the
deadline is the shorter of `--hours` and the cost cap over the price; a daily cap refuses an
`up`).

## Acceptance

1. With the test-double provider configured as `kind = "command"`, `slot node up <template>`
   prints the node's name and deadline; `slot node list` shows it Joining, then Ready once the
   stand-in daemon answers.
2. A second `up` with the same request key (a retried call) launches nothing: the double's log
   shows one provision.
3. `slot node down <name>` drains, terminates through the provider, deletes
   `<state-dir>/nodes/<name>/` key files, and a second `down` exits 0 and changes nothing.
4. An `up` whose hours exceed the provider's `max_hours`, or whose cost would pass the daily cap,
   exits 1 with the refusal's name before the provider is called.
5. A provider answer without `terminate_at`, or the double answering SelfTerminationNotArmed,
   leaves no node registered and exits 1.
6. `slot node list` names every machine the provider's `describe` returns that the registry does
   not know.
7. The `aws` provider builds its `aws` CLI arguments only from the host configuration; a test with
   a stub `aws` on `PATH` records the arguments and finds every value from the test's
   `cluster.toml` and none from the source. No test calls a cloud or the network.
8. `task public-safety` (from `story:cluster-design-and-spec-in-the-gate`) passes on the change.
9. `ess verify conform run` of `slot.provisioning` against the test double passes, and the
   conformance run of `story:daemon-conforms-to-cluster-spec` stays green.
10. Tests: each under 60 s, the step under 120 s.

## Scope

`crates/slot/src/provider.rs` (new), `crates/slot/src/provider/aws.rs` (new),
`crates/slot/src/server/nodes.rs`, `crates/slot/src/cli.rs`, `crates/slot/src/server/http.rs`,
`contrib/slot-provider-double` (new, POSIX sh), `website/docs/cluster/providers.md` (new).

## Not in this story

The boot script (`story:node-boot-script`); spot (`story:spot-nodes-and-reclaim`). An operator's
account values are configuration on the host, not work in this repository.
