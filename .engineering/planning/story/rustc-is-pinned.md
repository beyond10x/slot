---
format: aep.planning-md/3
id: story:rustc-is-pinned
kind: story
status: implemented
title: The local gate and CI compile with one rustc named in the repository
relations:
- serves: vision:O2
scope:
- confidence: cited
  path: .github/workflows/gate.yml
- confidence: inferred
  path: .github/workflows/pages.yml
- confidence: cited
  path: .github/workflows/release.yml
- confidence: cited
  path: AGENTS.md
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: Taskfile.yml
- confidence: inferred
  path: crates/slot-docs/tests/docs_version.rs
- confidence: cited
  path: rust-toolchain.toml
revision: 12
transitions:
- {from: "draft", to: "proposed", at: "2026-11-22T18:26:41Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-22T18:26:42Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-22T20:50:26Z", actor: "human:timo", revision: 12, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome

The local gate and CI compile with one rustc, named in the repository.

## Context

- The repository has no `rust-toolchain.toml` (`git show origin/main:rust-toolchain.toml`: does
  not exist, 2026-11-22). The local gate and CI each use the rustc they find.
- Workspace rule "The local gate equals the forge": `rust-toolchain.toml` pins one rustc and the
  local gate and CI both read it. Another repository lost 48 minutes of merges to rustc 1.100.0
  on CI against 1.99.1 locally.
- `AGENTS.md` open item (m).

## Acceptance

1. `rust-toolchain.toml` at the repository root names one exact version (`channel = "1.x.y"`,
   not `stable`) with the components `rustfmt` and `clippy`.
2. `.github/workflows/gate.yml` and `.github/workflows/release.yml` install the toolchain from that
   file, not from a version written in the workflow; a test reads both workflows and fails when
   either names a rustc version of its own.
3. The gate prints `rustc --version` before it builds, locally and in CI, and the two lines name
   the pinned version (one CI run and one local `task check` quoted in the pull request).
4. `AGENTS.md` open item (m) is removed.
