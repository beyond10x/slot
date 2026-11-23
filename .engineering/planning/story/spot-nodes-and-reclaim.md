---
format: aep.planning-md/3
id: story:spot-nodes-and-reclaim
kind: story
status: draft
title: Spot nodes fall back to on-demand, and a reclaim drains the node and reruns its jobs locally
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
- depends_on: story:provider-contract-and-node-up-down
- depends_on: story:node-boot-script
- depends_on: story:node-leaves-without-stranding-jobs
scope:
- confidence: inferred
  path: contrib/node-boot.sh
- confidence: inferred
  path: contrib/node-boot.test.sh
- confidence: inferred
  path: crates/slot/src/provider
- confidence: inferred
  path: crates/slot/src/provider.rs
- confidence: inferred
  path: crates/slot/src/server/poll.rs
revision: 5
---
# Story: spot nodes, with on-demand fallback and a clean reclaim

## Outcome

A template may ask for spot machines with the price capped at on-demand. When the provider finds
no spot capacity within a minute it launches on demand. A reclaim notice closes the node's
admission, the node drains, and jobs lost to the reclaim run once more on the local node.

## Specification

`spec/domains/provisioning.yaml` ProvisionMachine (`spot`, NoCapacity),
`spec/domains/cluster.yaml` DrainNode, MarkNodeGone reason Reclaimed, EndJob how NodeLost.

## Design

`docs/design/remote-pool.md` § 7.2, § 3.5 (spot notice row), § 7.3 step 12.

## Acceptance

1. The provision request a spot template sends records `spot` true and a price cap equal to the
   provider's on-demand price (the double's log shows both).
2. With the test-double provider answering NoCapacity for spot, `slot node up` sends one
   on-demand request within 60 s of that answer (timestamps in the double's log) and says so in
   one line.
3. A simulated notice on the stand-in node (the watcher's trigger file) closes its admission; the
   next poll shows the node Draining.
4. Ending the stand-in after the notice makes the node Gone with reason Reclaimed, and each job
   that was running is rerun once on the local node.
5. The conformance run stays green.
6. Tests: no network; each under 60 s, the step under 120 s.

## Scope

`crates/slot/src/provider.rs`, `crates/slot/src/provider/aws.rs`,
`crates/slot/src/server/poll.rs`, `contrib/node-boot.sh`, `contrib/node-boot.test.sh`.

`contrib/node-boot.sh` is also edited by `story:warm-caches-across-node-lives`, which depends on
this story.
