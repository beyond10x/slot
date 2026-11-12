---
format: aep.planning-md/3
id: story:atlas-catalog-adoption
kind: story
status: draft
title: The Atlas catalog holds slot, its release unit and components
relations:
- decomposes: epic:publish-first-release
- serves: vision:O6
- depends_on: story:public-repository
revision: 2
---
# Story: atlas-catalog-adoption

## Outcome

The Atlas catalog holds slot as a repository, its release unit, its components (`b10x-slot`,
`b10x-slot-cli`, `b10x-slot-docs`) and its relations; atlas `README.md` has a row for it and atlas
`ROADMAP.md` names the objectives it serves (O6, O2), as atlas `ROADMAP.md` § *How this page is
kept true* requires for a new repository.

## Context

- slot's `AGENTS.md` § Serves already states O6 and O2; the Atlas side is missing
  (`grep -n slot atlas/README.md` finds nothing at adoption).
- The change lands in the atlas repository through its steward session, not here. Atlas's catalog
  check is whatever that repository's gate runs on the commit; this story names the observable
  result, not atlas's command.
- Depends on `story:public-repository` (the catalog records the public repository URL).

## Acceptance

On the atlas commit that adopts slot:

1. `atlas/catalog/store/subjects/` holds a repository subject named `slot`, its release unit, and a
   component subject for each of `b10x-slot`, `b10x-slot-cli` and `b10x-slot-docs` (file names are
   hex-encoded; `grep -rl '"slot"' catalog/store/subjects` finds the repository and release unit).
2. `docs/catalog.md` § Repositories and atlas `README.md` each have a line naming `slot`.
3. atlas `ROADMAP.md` has an arrow row naming slot with `O6` and `O2`.
4. atlas's gate on that commit concludes `success`.

## Scope

- No change in this repository.
