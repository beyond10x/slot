---
format: aep.planning-md/3
id: story:node-boot-script
kind: story
status: draft
title: A POSIX sh boot script prepares a node and arms its end at the deadline first
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
- depends_on: story:cluster-design-and-spec-in-the-gate
scope:
- confidence: inferred
  path: Taskfile.yml
- confidence: inferred
  path: contrib/node-boot.sh
- confidence: inferred
  path: contrib/node-boot.test.sh
- confidence: inferred
  path: website/docs/cluster/node-boot.md
revision: 5
---
# Story: a node boot script that ends the machine at its deadline

## Outcome

A POSIX sh boot script that a provider hands to a new machine prepares it as a remote node and
arms its end first: the machine stops at its deadline and after 30 idle minutes, whether or not
the workstation is on.

## Specification

`spec/domains/provisioning.yaml` (self-termination armed before the answer; `terminate_at`),
`spec/domains/cluster.yaml` Node (`slot_version`, `deadline`).

## Design

`docs/design/remote-pool.md` § 7.3 steps 1 to 13, § 8.3 (no route from build jobs to the
instance's metadata service), § 8.7 layer 1.

## Acceptance

1. `contrib/node-boot.sh` takes the deadline, the slot version, the public key and the
   toolchains as arguments or environment; `task public-safety` passes on it.
2. Run with every system command replaced by a recording stub, its first recorded call is
   `shutdown` with the deadline it was given (a deadline 240 minutes ahead records `+240`).
3. The idle watchdog it installs stops the machine after 30 minutes without a job: the recorded
   unit or timer names 30 minutes, and a test that feeds the watchdog an idle state of 31
   minutes records a `shutdown -h now`.
4. It installs the slot release of the given version and refuses one whose checksum does not
   match `SHA256SUMS` (a test with a tampered archive).
5. The sshd settings of step 6, the build user without sudo and the metadata block by user id
   appear in the recorded calls.
6. `shellcheck` passes on it; it runs under `dash`.
7. The tests are a POSIX sh script, `contrib/node-boot.test.sh`, run by `task boot-script-test`
   inside `task check`; no network; under 60 s.

## Scope

`contrib/node-boot.sh` (new), `contrib/node-boot.test.sh` (new), `Taskfile.yml`,
`website/docs/cluster/node-boot.md` (new).
