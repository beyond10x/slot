---
format: aep.planning-md/3
id: story:eviction-starts-where-the-disk-hold-starts
kind: story
status: implemented
title: Eviction runs whenever the disk hold stops builds from starting
relations:
- serves: vision:O2
scope:
- confidence: cited
  path: AGENTS.md
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: crates/slot/src/admission.rs
- confidence: cited
  path: crates/slot/src/cli.rs
- confidence: cited
  path: crates/slot/src/evict.rs
- confidence: cited
  path: crates/slot/src/server/mod.rs
- confidence: cited
  path: crates/slot/src/server/tests.rs
- confidence: cited
  path: crates/slot/src/status.rs
- confidence: cited
  path: docs/design.md
- confidence: inferred
  path: ui/src/generated/types.ts
- confidence: inferred
  path: ui/ux/scenarios.ts
- confidence: cited
  path: website/docs/how-it-works.md
- confidence: cited
  path: website/docs/reference/cli.md
revision: 17
transitions:
- {from: "draft", to: "proposed", at: "2026-11-22T18:26:43Z", actor: "agent:slot", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-22T18:26:43Z", actor: "agent:slot", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-22T20:50:27Z", actor: "agent:slot", revision: 17, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome

When the disk hold stops builds from starting, eviction is already working to free the space
they wait for.

## Context

- `slot serve --min-free-disk` holds every build while a filesystem has less free space than it;
  `--evict-below` (default 40 GB) starts eviction (`crates/slot/src/cli.rs`, `evict_below`).
- Measured 2026-11-22 with `--min-free-disk 50`: free space fell from 77 GB to 44 GB, for about
  12 minutes up to 38 jobs were queued for disk while 2 ran, and eviction did not start. The queue
  emptied when sessions deleted their own build output (about 31 GB).
- Both values are options of `slot serve`; a change needs a full restart, which drains (open item
  (g)).
- `AGENTS.md` open item (ab).

## Acceptance

1. Test: with `--min-free-disk 50` and `--evict-below 40`, a filesystem at 45 GB free gets an
   eviction pass, and the job held for disk starts once the pass frees enough; the mark eviction
   uses is the larger of the two values.
2. `slot status` prints the eviction mark in force and, when it was raised to the disk hold's
   value, says so in the same line.
3. `--evict-below 0` still turns eviction off, whatever `--min-free-disk` is (test).
4. `docs/design.md`, `website/docs/how-it-works.md` and the CLI reference (`task generate`) state
   the rule. `AGENTS.md` open item (ab) is removed.
