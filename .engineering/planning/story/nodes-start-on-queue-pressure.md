---
format: aep.planning-md/3
id: story:nodes-start-on-queue-pressure
kind: story
status: draft
title: Nodes start on queue pressure and stop when idle, within the operator's budget
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
- depends_on: story:provider-contract-and-node-up-down
- depends_on: story:results-come-back-from-a-node
scope:
- confidence: inferred
  path: crates/slot/src/hostconfig.rs
- confidence: inferred
  path: crates/slot/src/server/autoscale.rs
- confidence: inferred
  path: crates/slot/src/server/mod.rs
- confidence: inferred
  path: spec/domains/hostconfig.yaml
revision: 4
---
# Story: nodes start and stop on queue pressure, within the operator's budget

## Outcome

With automatic start switched on in the host configuration, the daemon launches a node of a named
template when the local queue wait stays over a mark, and takes it down when it idles; it never
passes the daily budget or the most hours per node the configuration sets.

## Specification

`spec/domains/hostconfig.yaml` gains an `[auto]` table in this story: template, queue-wait mark,
minutes over the mark, daily budget, most hours per node, idle minutes; with its refusals (a
template not declared, a budget or hours not positive). `spec/domains/provisioning.yaml`
ProvisionMachine (CostCapReached), `spec/domains/cluster.yaml` DrainNode.

## Design

`docs/design/remote-pool.md` § 7.5 (automatic start only in phase 3, only under a budget set
beforehand), § 9.5 (queue wait p90).

## Acceptance

1. The spec declares `[auto]` and its refusals; `ess specify validate --path spec` passes and the
   conformance run stays green; `slot cluster config check` reports an `[auto]` naming an
   undeclared template.
2. With `[auto]` absent nothing starts by itself.
3. Against the test-double provider and a simulated queue whose p90 wait stays over the mark for
   the configured minutes, the daemon runs one `up`; with the day's spend at the budget it runs
   none and logs one line.
4. Every node it starts has a deadline no later than the most hours per node; a node of a
   template whose hours exceed that bound is started with the bound as its lifetime, shown in the
   double's log.
5. A node idle for the configured minutes is drained and taken down.
6. Tests: no network; each under 60 s, the step under 120 s.

## Scope

`crates/slot/src/server/autoscale.rs` (new), `crates/slot/src/server/mod.rs` (the module line and
its start), `crates/slot/src/hostconfig.rs`, `spec/domains/hostconfig.yaml`.

`crates/slot/src/hostconfig.rs` and `spec/domains/hostconfig.yaml` are also edited by
`story:warm-caches-across-node-lives`, which depends on this story.
