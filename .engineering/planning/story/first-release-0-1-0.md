---
format: aep.planning-md/3
id: story:first-release-0-1-0
kind: story
status: draft
title: Release 0.1.0 publishes two Linux archives and SHA256SUMS
relations:
- decomposes: epic:publish-first-release
- serves: vision:O6
- depends_on: story:gates-enrolment
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: Cargo.toml
revision: 4
---
# Story: first-release-0-1-0

## Outcome

Tag `0.1.0`, pushed by `b10x-bot[bot]` through `b10x-gates bot --repository beyond10x/slot`, runs
`.github/workflows/release.yml` to completion: the tag, `Cargo.toml` and `CHANGELOG.md` agree, and
GitHub release 0.1.0 carries `slot-0.1.0-x86_64-unknown-linux-gnu.tar.gz`,
`slot-0.1.0-aarch64-unknown-linux-gnu.tar.gz` and `SHA256SUMS` (`AGENTS.md` § Release).
`CHANGELOG.md` § 0.1.0 carries a date instead of "unreleased".

## Context

- Two Linux targets only: slot reads `/proc` and has no macOS port.
- Depends on `story:gates-enrolment`.

## Acceptance

1. `gh api 'repos/beyond10x/slot/actions/workflows/release.yml/runs?branch=0.1.0&event=push' --jq '.workflow_runs[0] | [.actor.login, .conclusion] | join(" ")'`
   prints `b10x-bot[bot] success`.
2. `gh release view 0.1.0 --repo beyond10x/slot --json assets --jq '.assets[].name'` lists exactly
   the two archives and `SHA256SUMS`.
3. After downloading them, `sha256sum -c SHA256SUMS` passes, and `slot --version` from the
   x86_64 archive prints `0.1.0`.
4. `git show 0.1.0:CHANGELOG.md | grep -E '^## 0\.1\.0 — [0-9]{4}-[0-9]{2}-[0-9]{2}$'` matches one
   line.
5. `cargo install --git https://github.com/beyond10x/slot --tag 0.1.0 --locked b10x-slot-cli`
   succeeds on a machine with no checkout (the install line in `README.md`).

## Scope

- `CHANGELOG.md` (release date)
- `Cargo.toml` (version, if it moves)
- `.github/workflows/release.yml` only if the first run exposes a defect
