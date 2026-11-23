---
format: aep.planning-md/3
id: story:cluster-conformance-runs-in-the-gate
kind: story
status: draft
title: The cluster's conformance suite runs against the daemon in the gate
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
- depends_on: story:cluster-types-generated-from-the-spec
scope:
- confidence: inferred
  path: Taskfile.yml
- confidence: inferred
  path: crates/slot/src/conformance.rs
- confidence: inferred
  path: crates/slot/tests/conformance.rs
- confidence: inferred
  path: crates/slot/tests/main.rs
- confidence: inferred
  path: spec/conformance/implemented.txt
- confidence: inferred
  path: spec/generated/conformance
revision: 2
---
# Story: the cluster's conformance suite runs against the daemon in the gate

## Outcome

The suite ESS synthesizes from `spec/` is committed, a conformance target built from the
daemon's own code answers it, and `task check` runs the part of it the daemon implements. The
part grows with each daemon story; the run never asks for a scenario the daemon has not built
yet, and never passes one it fails.

## Specification

`spec/` (today 109 synthesized scenarios, 46 refusals). The suite is the
`slot-daemon` component's: `ess verify conform synthesize --path spec --component slot-daemon`.

## Design

- The full suite is committed as `spec/generated/conformance/suite.json` (`--compact`,
  `--suite-format 5`, the format `select` narrows) and held by
  `ess verify conform synthesize … --check`, like every other projection.
- The gate runs a selection: `spec/conformance/implemented.txt` lists the scenario ids the daemon
  implements, one per line; `ess verify conform select` narrows the committed suite to them, and
  `ess verify conform run --target process --target-timeout 10 --report-format 2` runs the
  selection against the target. An id in the list that the suite no longer holds fails the gate.
- The target is a test-only binary of `crates/slot` that answers `ess-conformance-target/1` on
  stdin and stdout from the daemon's own node, placement and job types, in memory: no socket, no
  network, no state directory. It is a module of the crate's one integration-test binary
  (`crates/slot/tests/main.rs`) started by that test, so the gate adds no test binary.
- From this story on, a story that implements a command or view adds its scenario ids to
  `implemented.txt` in the same change. `story:daemon-conforms-to-cluster-spec` closes when the
  list holds every synthesized scenario of `slot.cluster` and `slot.hostconfig`.
- ESS skill `ess:testing-conformance`.

## Acceptance

1. `spec/generated/conformance/suite.json` is committed and its `--check` passes in `task check`;
   a spec change without regenerating fails it.
2. With `implemented.txt` empty the run executes no scenario and passes; with the NodeList view's
   scenarios listed, the target answers them from the daemon's types (the shape of
   `GET /api/nodes`) and they pass.
3. A planted defect in the code that answers NodeList (a field dropped) makes the run fail, shown
   once in the pull request; an id in `implemented.txt` that the suite lacks fails the gate.
4. The run's report is recorded with `aep plan artifact evidence
   executable-system-specification:compute-cluster --from <report>`.
5. The run fits the budget: each scenario under 60 s (the target answers in under 10 s or the
   run fails), the whole conformance step under 30 s, `task check`'s test step under 120 s; the
   step prints its ten slowest scenarios.
6. CI and the local gate use the `ess` release `spec/ess-inputs.yaml` pins.

## Scope

`spec/generated/conformance/`, `spec/conformance/implemented.txt` (new),
`crates/slot/tests/main.rs`, `crates/slot/tests/conformance.rs` (new),
`crates/slot/src/conformance.rs` (new, test-only target), `Taskfile.yml`.

## Not in this story

Implementing the cluster's commands (the daemon stories, each adding its ids); covering the
refusals synthesis cannot arrange (`story:daemon-conforms-to-cluster-spec`).
