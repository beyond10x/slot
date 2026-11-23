---
format: aep.planning-md/3
id: story:host-config-file
kind: story
status: draft
title: The host's cluster.toml declares local share, providers and templates, and slot cluster config check reports every broken rule
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
- depends_on: story:cluster-design-and-spec-in-the-gate
- depends_on: story:cluster-types-generated-from-the-spec
- depends_on: story:cluster-conformance-runs-in-the-gate
scope:
- confidence: inferred
  path: crates/slot/src/cli.rs
- confidence: inferred
  path: crates/slot/src/conformance.rs
- confidence: inferred
  path: crates/slot/src/hostconfig.rs
- confidence: inferred
  path: crates/slot/src/lib.rs
- confidence: inferred
  path: crates/slot/src/paths.rs
- confidence: inferred
  path: spec/conformance/implemented.txt
- confidence: inferred
  path: website/docs/reference/cli.md
- confidence: inferred
  path: website/docs/reference/cluster-config.md
revision: 5
---
# Story: the host configuration file, validated, with `slot cluster config check`

## Outcome

A TOML file of the endpoint's machine, `$XDG_CONFIG_HOME/slot/cluster.toml` (outside every
repository), declares the local node's share, the providers and the node templates. The daemon
reads it at start and at reload; `slot cluster config check` reports every rule it breaks and
writes nothing.

## Specification

`spec/domains/hostconfig.yaml`: HostConfig, Provider, NodeTemplate; commands LoadHostConfig,
LoadProvider, LoadNodeTemplate, RemoveProvider, RemoveNodeTemplate and each refusal
(PathNotAbsolute, InsideRepository, ShareOutOfRange, LimitNotPositive, SettingsDoNotMatchKind,
SizeNotPositive, ProviderNotDeclared, HoursOverProviderMax, ProviderInUse); views HostConfigs,
Providers, NodeTemplates.

## Design

`docs/design/remote-pool.md` § 7.5 (who starts a node, the provider program), § 8.7 layer 3
(hours, cost cap), § 3.2 rule 3 (operator's state, not a repository file). Operator's decision
of 2026-11-23: account values only in this file.

## Acceptance

1. With no file, the daemon starts as today and the local share is 50 % of the CPUs; `slot
   cluster config check` prints `no cluster.toml at <path>; defaults apply` and exits 0.
2. A file with `[node.local] cpu_percent = 50`, one `[provider.<name>] kind = "command"` with an
   absolute `path`, and one `[template.<name>]` loads; `check` prints each table and exits 0.
3. One fixture per refusal of `slot.hostconfig` makes `check` exit 1 and print the refusal's name
   and the table, all refusals of a file in one run: a relative path, `cpu_percent = 0`,
   `max_hours = 0`, `kind = "aws"` without an `[provider.<name>.aws]` table, both tables, a
   template naming an undeclared provider, a template's `hours` over its provider's
   `max_hours`, and the file inside a git working tree.
4. A key the shape does not declare (for example `secret_access_key`) is refused by name: every
   table is closed.
5. No view and no output prints the `aws` or `command` table's values; `check` prints the kind,
   the limits and the sizes only.
6. A name-free example file is in `website/docs/reference/cluster-config.md`, and a test parses
   it from the page.
7. The file is parsed into `ClusterFile` of `crates/slot-cluster-types` (generated from the spec); no
   hand-written struct of the file's shape exists in `crates/slot`.
8. The scenario ids of every `slot.hostconfig` command and view are added to
   `spec/conformance/implemented.txt`, and the gate's conformance run passes them.
9. Tests: no network; each under 60 s, the step under 120 s.

## Scope

`crates/slot/src/hostconfig.rs` (new), `crates/slot/src/cli.rs`, `crates/slot/src/paths.rs`,
`crates/slot/src/lib.rs`, `website/docs/reference/cluster-config.md`,
`spec/conformance/implemented.txt`, `crates/slot/src/conformance.rs`.

## Not in this story

Using a provider (`story:provider-contract-and-node-up-down`); the local node itself
(`story:daemon-knows-nodes`). An operator's account values are configuration on the host, not
work in this repository.
