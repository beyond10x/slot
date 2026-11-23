---
format: aep.planning-md/3
id: story:daemon-conforms-to-cluster-spec
kind: story
status: draft
title: The daemon passes every synthesized scenario of its cluster specification
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
- depends_on: story:node-leaves-without-stranding-jobs
- depends_on: story:cluster-design-and-spec-in-the-gate
- depends_on: story:node-boot-script
- depends_on: story:cluster-conformance-runs-in-the-gate
scope:
- confidence: inferred
  path: crates/slot/src/conformance.rs
- confidence: inferred
  path: crates/slot/tests/conformance.rs
- confidence: inferred
  path: spec/conformance/implemented.txt
- confidence: inferred
  path: spec/ess-inputs.yaml
- confidence: inferred
  path: spec/scenarios
revision: 9
---
# Story: the daemon passes every synthesized scenario of its cluster specification

## Outcome

`spec/conformance/implemented.txt` holds every scenario ESS synthesizes for `slot.cluster` and
`slot.hostconfig`, the gate's conformance run passes all of them, and the refusals synthesis
cannot arrange are covered by authored scenarios or named as not covered. From this story on a
spec change keeps the full run green in the same change.

## Specification

`spec/domains/cluster.yaml` and `spec/domains/hostconfig.yaml` as they stand when this story
merges. Today: 109 scenarios, 46 refusals (`executable-system-specification:compute-cluster`).
`spec/domains/provisioning.yaml` is held by `story:provider-contract-and-node-up-down` against
its test-double provider.

## Design

The run, the target and `implemented.txt` come from `story:cluster-conformance-runs-in-the-gate`;
the daemon stories before this one each added the ids of what they built.
`docs/design/remote-pool.md` § 9.2 (tests: placement table, handover, lost node). ESS skill
`ess:testing-conformance`.

## Acceptance

1. `implemented.txt` equals the list of every synthesized scenario id of `slot.cluster` and
   `slot.hostconfig` in the committed suite (a test compares them), and the run passes.
2. Each refusal listed by `ess verify conform synthesize --path spec` for those two domains is
   either covered by an authored scenario in `spec/scenarios/` or named with its code in the body
   of `executable-system-specification:compute-cluster` as not covered and why.
3. A planted defect (SubmitJob ignoring placed-elsewhere) makes the run fail, shown once in the
   pull request.
4. The report is recorded with `aep plan artifact evidence
   executable-system-specification:compute-cluster --from <report>`.
5. The conformance step stays under 30 s and `task check`'s test step under 120 s, each scenario
   under 60 s.

## Scope

`spec/conformance/implemented.txt`, `spec/scenarios/` (new), `spec/ess-inputs.yaml`,
`crates/slot/src/conformance.rs`, `crates/slot/tests/conformance.rs`.
