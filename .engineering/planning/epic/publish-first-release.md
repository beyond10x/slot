---
format: aep.planning-md/3
id: epic:publish-first-release
kind: epic
status: draft
title: slot is public and released as 0.1.0
summary: Public repository, gates, Atlas catalog, release 0.1.0 for two Linux targets, live docs site, agentplugins slot plugin.
relations:
- serves: vision:O6
- informed_by: specification:delivered-baseline
revision: 3
---
# Epic: slot is public and released as 0.1.0

## Outcome

`beyond10x/slot` is a public GitHub repository, enrolled in the organization's gates, cataloged in
Atlas, with release 0.1.0 published for two Linux targets, the documentation site live at
<https://beyond10x.github.io/slot/>, and the agentplugins `slot` plugin released so an agent
session can install both the CLI and the instruction to use it.

## Why

O6: the job log measures only builds that run through slot, and sessions on other machines can
install slot only from a release. Today the repository has no commit and no remote
(`CHANGELOG.md` § 0.1.0 "unreleased"; `README.md` § Install already points at
`github.com/beyond10x/slot`, which does not exist yet).

## Stories

1. `story:public-repository` — the repository exists, public.
2. `story:gates-enrolment` — gates enrolled; `b10x-bot[bot]` pushes the first commit; the gate
   runs green on `main`.
3. `story:atlas-catalog-adoption` — Atlas catalogs slot (repository, release unit, components, arrows).
4. `story:first-release-0-1-0` — tag 0.1.0 publishes two Linux archives and `SHA256SUMS`.
5. `story:docs-site-live` — the site answers at its public URL, deployed from `main`.
6. `story:agentplugins-slot-plugin-released` — agentplugins release carries the `slot` plugin.

Order: 1 → 2 → 4 → 6; 3 needs 1; 5 needs 1 and 2 (each story names its `depends_on`).

## Not in this epic

Any change to slot's behaviour. Release automation beyond what `.github/workflows/release.yml`
already does.

## Acceptance

The epic is done when all of these hold at once: `gh api repos/beyond10x/slot --jq .visibility`
prints `public`; `gh release view 0.1.0 --repo beyond10x/slot` lists
`slot-0.1.0-x86_64-unknown-linux-gnu.tar.gz`, `slot-0.1.0-aarch64-unknown-linux-gnu.tar.gz` and
`SHA256SUMS`; `curl -fsS https://beyond10x.github.io/slot/` exits 0; and the current agentplugins
release's marketplace manifest lists a plugin named `slot`.
