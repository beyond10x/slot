---
format: aep.planning-md/3
id: story:ux-loop-web-ui
kind: story
status: draft
title: Every change to the web UI passes the UX gates and never makes a measured value worse
summary: 'Standing story of the UI iteration loop: one task per round holds its change set.'
relations:
- serves: vision:O6
- decomposes: epic:adoption-measured
revision: 2
---
# Story: ux-loop-web-ui

## Outcome

The operator watching builds in the web UI can do every task the scenarios name (open a failed
build's output, set headroom for a call, find a queued job's position) by pointer or keyboard, and no
merge makes a measured UX value worse.

## Context

The web UI grew panel by panel (host census, headroom slider) without a check on layout, keyboard
access or false claims. The UI iteration loop adds hermetic scenarios, hard gates on every merge
(`ui/ux/gates.spec.ts`, `ui/ux/screen-only.spec.ts`) and a ratchet (`ui/ux/baseline.json`,
`ui/ux/ratchet.ts`). Each round's change set is one task under this story.

## Acceptance

- `task check` runs the UX gates: no sideways page scroll, always-visible header regions, axe-core 0
  serious or critical violations, every screen-only task done by pointer and keyboard, at most 2
  actions per fix, `false_complete` and `completion_miss` 0, at 1366x768 and 1280x720.
- `node ux/ratchet.ts` exits 0 against `ui/ux/baseline.json`.
- Every closed S1 or S2 record in `ui/ux/findings/` names a regression asset that exists
  (`ui/ux/findings.test.ts`).

## Out of Scope

- Keyed replay of recorded sessions (L4), the discoverability probe (L5) and rounds with people (L6):
  slot has no recorded sessions.
- A dark theme.

## Ambiguities

- None open.

## Open Questions

- None.
