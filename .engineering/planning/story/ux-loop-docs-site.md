---
format: aep.planning-md/3
id: story:ux-loop-docs-site
kind: story
status: draft
title: Every change to the docs site passes the docs UX gates and never makes a measured value worse
summary: 'Standing story of the docs iteration loop: one task per docs round holds its change set.'
relations:
- serves: vision:O6
- decomposes: epic:adoption-measured
revision: 2
---
# Story: ux-loop-docs-site

## Outcome

A reader of the slot docs site finds how to install slot, keep CPU free for a call, cancel a job,
forward environment variables to a remote server, upgrade the daemon, what slot does on a full disk,
and how to add the Claude Code plugin, from the landing page by pointer or keyboard, and no merge
makes a measured docs UX value worse.

## Context

The docs site grew page by page with the releases (0.1.0 to 0.10.0) while the landing page kept the
0.1.0 eyebrow and status list. The docs iteration loop (`website/ux/`) adds the reader tasks, hard
gates on every merge (`gates.spec.ts`, `screen-only.spec.ts`, `copy.test.ts`) and a ratchet
(`website/ux/baseline.json`) that reuses `ui/ux/ratchet-lib.ts`. Each docs round's change set is one
task under this story.

## Acceptance

- `task check` runs `task site` and `task site-ux`: at 1366x768 and 1280x720 in both themes, no
  sideways page scroll, nothing past its box, navbar, search, theme toggle and sidebar in view at
  the top and the end of the content, axe-core 0 serious or critical violations; every reader task
  done by pointer and keyboard in at most 3 actions.
- `node ux/ratchet.ts` in `website/` exits 0 against `website/ux/baseline.json`.
- `website/ux/copy.test.ts` passes: the landing page names no version but the workspace's, and a
  planned status item points at a "Not built yet" section that still lists it.
- Every closed S1 or S2 record in `website/ux/findings/` names a regression asset that exists.

## Out of Scope

- Keyed replay (L4), the discoverability probe (L5) and rounds with people (L6).
- The link styling and search of the shared docs system: changed in this site only.
