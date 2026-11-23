---
format: aep.planning-md/3
id: story:warm-caches-across-node-lives
kind: story
status: draft
title: Dependency caches and toolchains stay warm across node lives
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
- depends_on: story:node-boot-script
- depends_on: story:provider-contract-and-node-up-down
- depends_on: story:spot-nodes-and-reclaim
- depends_on: story:nodes-start-on-queue-pressure
- depends_on: story:one-hand-started-node-measured
scope:
- confidence: inferred
  path: contrib/node-boot.sh
- confidence: inferred
  path: crates/slot/src/hostconfig.rs
- confidence: inferred
  path: docs/design/remote-pool.md
- confidence: inferred
  path: spec/domains/hostconfig.yaml
- confidence: inferred
  path: website/docs/cluster/providers.md
revision: 5
---
# Story: caches stay warm across node lives

## Outcome

A node started tonight compiles dependency crates it compiled last night from a shared compile
cache, and boots from an image with toolchains and a warm registry, so a worktree's first build
on a new node is not cold.

## Specification

`spec/domains/hostconfig.yaml`: the `image` of `AwsSettings` (declared already) names the baked
image, and this story adds a cache location (values only in the host configuration);
`spec/domains/provisioning.yaml`.

## Design

`docs/design/remote-pool.md` § 5.6 (sccache across workspace ids: the open cause; the object-store
backend; a per-host cache until the cause is understood), § 7.3 (baked image), § 8.6 (a poisoned
shared cache: a bucket per night, or the risk accepted in writing), § 9.5 (cold starts).

## Acceptance

1. The cause of the poor hit rate across workspace ids (§ 5.6) is measured on a node and written
   into the design, with the change that fixes it if one does.
2. The spec declares the cache location beside the `image` of `AwsSettings` and validates; the
   conformance run stays green; `task public-safety` passes.
3. With the cache location configured, a second node life's first build of a worktree at an
   unchanged dependency set shows cache hits for the dependency crates in `sccache
   --show-stats`, recorded in a verification-report.
4. A node booted from the image has the toolchains the template names on `PATH` at the versions the boot
   script was given, and `cargo fetch --offline` of a worktree at an
   unchanged dependency set succeeds (the registry is warm); its time to Ready is under one
   minute. Each is recorded in the verification-report.
5. The operator's decision on the shared-cache risk of § 8.6 is recorded in this story's body
   before the shared cache is switched on.

## Scope

`contrib/node-boot.sh`, `crates/slot/src/hostconfig.rs`, `spec/domains/hostconfig.yaml`,
`docs/design/remote-pool.md`, `website/docs/cluster/providers.md`.

Ordered after `story:spot-nodes-and-reclaim` (`contrib/node-boot.sh`) and
`story:nodes-start-on-queue-pressure` (`hostconfig.rs`, `spec/domains/hostconfig.yaml`).
