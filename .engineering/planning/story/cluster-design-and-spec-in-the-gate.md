---
format: aep.planning-md/3
id: story:cluster-design-and-spec-in-the-gate
kind: story
status: draft
title: The cluster's design and specification are committed, name no account value, and the gate validates the spec
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
scope:
- confidence: inferred
  path: .github/workflows/gate.yml
- confidence: inferred
  path: Taskfile.yml
- confidence: inferred
  path: contrib/public-safety.patterns
- confidence: inferred
  path: docs/design/assets
- confidence: inferred
  path: docs/design/remote-pool.md
- confidence: inferred
  path: spec
revision: 5
---
# Story: the cluster's design and specification are committed and checked by the gate

## Outcome

`docs/design/remote-pool.md` and `spec/` are on `main`, the design names no account value of a
cloud, and `task check` and CI refuse a specification that does not validate and a tree that
names an account value.

## Specification

All of `spec/` (`executable-system-specification:compute-cluster`).

## Design

`docs/design/remote-pool.md` as reviewed on 2026-11-23, with three changes the operator's
decisions of 2026-11-23 require: the spec's vocabulary (node, `slot node …`, placement), the
local node at half the CPUs (§ 3, § 3.2), and provisioning through the provider contract and the
host configuration (§ 7.5, § 8): the runbook of § 9.1 keeps its steps and loses its account
values and its region choice.

## Acceptance

1. A committed pattern file, `contrib/public-safety.patterns`, holds one extended regular
   expression per line, at least these (matched case-insensitively):
   `\b(us|eu|ap|sa|ca|me|af|il|mx)-(north|south|east|west|central|northeast|northwest|southeast|southwest)-[0-9]\b`,
   `\bus[ -]east\b`, `\b[0-9]{12}\b`, `\bami-[0-9a-f]{8,}\b`, `\bsubnet-[0-9a-f]{8,}\b`,
   `\bsg-[0-9a-f]{8,}\b`, `\bvpc-[0-9a-f]{8,}\b`, `\barn:aws[a-z-]*:`, `\bAKIA[0-9A-Z]{16}\b`,
   `/home/[a-z]`, `^\s*profile\s*=\s*"[^<]`.
2. `task public-safety` runs `git grep -n -I -i -E -f contrib/public-safety.patterns -- docs spec
   website/docs crates contrib ':!contrib/public-safety.patterns'` and exits 1 on a match;
   `task check` runs it. It exits 0 on the tree of this story, and 1 on a commit that adds
   `region = "eu-west-1"` to a docs page (shown once in the pull request). Instance types and the
   public `aws` CLI may be named as examples.
3. The design uses node, `slot node …` and placement as the spec does; `grep -c 'slot host '`
   over it is 0.
4. `task check` runs `ess specify validate --path spec`; a specification with a broken reference
   (a `target:` naming no entity) makes `task check` exit non-zero, shown once in the pull
   request.
5. The CI gate runs the same steps with an `ess` release pinned in `spec/ess-inputs.yaml`
   (`format: ess-inputs/2`, `requires: ess <version>`), so the local gate and CI read one `ess`.

## Scope

`docs/design/remote-pool.md`, `spec/`, `contrib/public-safety.patterns`, `Taskfile.yml`,
`.github/workflows/gate.yml`.

`.github/` is also in the scope of `story:gates-enrolment` (another epic); this story changes only
the step list of the gate job, and whichever merges second rebases onto the other.

## Not in this story

Conformance of the daemon to the spec (`story:daemon-conforms-to-cluster-spec`, which depends on
this one for `Taskfile.yml` and `spec/ess-inputs.yaml`).
