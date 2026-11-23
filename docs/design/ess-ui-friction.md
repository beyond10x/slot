# Friction with a UI generated from the ESS UI spec

What got in the way while the job list and the job detail were built from `spec/ui/slot.ui.yaml`
(`ess-ui/1`, ESS 0.98.0). Each entry: where in the UI spec, the change wanted, what the generator
or the format did, the workaround, and the cost (minutes, files, lines). The ESS-side story for
each gap is in [ess-ui-gaps.md](ess-ui-gaps.md) (G-numbers).

## Path taken

1. `ess generate ui --target react` (the stock generator) into a scratch `ui-ess/`. It built and
   ran against the daemon's JSON. Entry F5 was the first point that needed a change on the ESS
   side (a value the spec cannot tell the generator to draw differently), so the stock generator
   was dropped there, by the rule set for this work: stop at the first such friction. Its output
   was not committed.
2. slot's own generator: `slot-docs ui-pages` (crates/slot-docs) reads the same `ess-ui/1`
   document and `spec/http.yaml` and writes Vue pages into `ui/src/generated/pages/`, drawn by the
   hand-written components of `ui/src/components/spec/` in the look of the existing job tables.
   `ess ui check` still checks the document against the specification. Entries F14 to F18 are what
   the format itself still costs with our own generator.

Stock generator time from first spec line to the stop: about 45 minutes, about 20 of them lost
to the entries below.

## Entries

| # | Where (UI spec node path) | Wanted | What happened | Workaround | Cost |
|---|---|---|---|---|---|
| F1 | `spec/http.yaml`, view `slot.pool.Jobs` | read the jobs where the daemon already sends them: `active` and `done` of `GET /api/state` | a view route answers `rows` (`{rows: […]}`), `array` or `single`; nothing names a member of a larger answer (G1) | two daemon routes, `GET /api/jobs` and `GET /api/jobs/{id}`, that answer as the spec binds them | 15 min; 3 files, about 70 lines (routes, method, test) |
| F2 | `spec/domains/pool.yaml`, `SubmitJob.outcomes.queued.sets` | set `cpu_s: 0.0` on a new job | a `Binary64` literal in `sets` is refused (G2) | left unset | 2 min; 0 lines |
| F3 | `spec/domains/pool.yaml`, entity `slot.pool.Job` | declare the job the daemon records and its states, read-only | every lifecycle transition needs a command outcome that takes it, and every outcome an event (`missing_causation`, `empty_change`) (G13) | four commands, four events and two errors that describe the daemon's socket protocol; no HTTP route | 10 min; 1 file, about 150 lines |
| F4 | `pages/jobs.list/sections/running/reads/filter` | one search over command, repository and session | `contains` reads one field; an `or` of three `contains` needs every word in one field (G8) | search over the command only | 3 min |
| F5 | `pages/jobs.list/sections/running/columns/id` (stock React) | the job number as written, `61259` | drawn as `61,259`, the locale's grouping of a number; no `as` or option draws an integer id without it (G3) | none in the spec. **Stop of the stock generator.** | 2 min |
| F6 | `pages/jobs.list/sections/*/columns/argv`, `as: list` (stock React) | the command line, items joined by spaces, monospace, one line cut with an ellipsis | `task, ux:image`: items joined by a comma, wrapped over several lines (G4) | none in the spec; slot's generator joins by spaces | 1 min |
| F7 | `pages/jobs.list/sections/*` (stock React) | the dense job table of the web view: fixed column widths, one line per job, 31 px rows, sticky headings | cells wrap, rows grow to three lines, the table runs past 1366 px and its last columns are cut; tokens set colours and spacing only, no class hook for the host's CSS (G9) | none in the spec; slot's component set holds the widths per field | 3 min |
| F8 | `pages/jobs.detail/sections/summary/groups/timing`, `as: timestamp` | the local wall-clock time, as every other view of the web view shows it | `time` and `timestamp` are UTC by definition (G6) | slot's generator draws local time: it deviates from the format's meaning | 2 min |
| F9 | `pages/jobs.detail/sections/summary` (stock React) | label and value side by side, dense, beside the output | one label above one value per line; the page is several screens long | none in the spec; slot's component set draws a two-column grid | 1 min |
| F10 | `pages/jobs.detail/sections/summary/groups/where/fields/checkout`, `as: map` (stock React) | `worktree fix-login` | `{"kind":"worktree","name":"…"}`, the raw JSON of the tagged union (G5) | none; the field was dropped from the detail page | 1 min |
| F11 | `pages/jobs.list/sections/*/columns` | wait and run time, each computed from two fields and the clock | a field shows one value of the row; nothing derives one (G10) | a host slot per column | 3 min |
| F12 | `pages/jobs.list/sections/*/columns/toolchain` | the toolchain's colour dot that every chart of the web view uses | a value is text, a badge or a tone; no colour per value outside the five tones (G11) | a host slot per column | 1 min |
| F13 | `pages/jobs.detail/header/badge` | the state badge toned like the state column | `badge.tone` is one tone for every value; no `tone_by` (G11) | neutral badge | 1 min |
| F14 | `pages/jobs.list/sections/{running,queued,done}/columns/{toolchain,repo,enqueued_ms,started_ms}` | one host component (`JobTool`) in every table that shows the column | a host name is unique in the document: `host_slots` refuses the second use, also through a YAML alias (G7) | eleven host names for four components (`RunningTool`, `QueuedTool`, `DoneTool`, …); `ui/src/hosts.ts` maps them | 8 min; 3 files, about 30 lines |
| F15 | `pages/jobs.detail/sections/summary/groups/*/fields/{toolchain,repo}` | the same tool and repository cells on the detail page | same as F14 | plain text there | 1 min |
| F16 | whole document (slot's generator) | read the document as `ess ui check` resolves it: page kinds merged, short forms expanded | `ess ui load` prints a summary only; no machine-readable resolved document (G12, already filed by the UI session) | slot's generator reads the YAML with serde, takes the part the pages use and refuses every other key by name; the page-kind merge is not applied, so the document restates what the kind would give (`state.search`, `remove: true`) | 10 min; 1 file, about 900 lines |
| F17 | `pages/*/sections/*/columns` (slot's generator) | column widths that hold the widest value | no width in a `Field` (G9) | widths per field name in `SpecTable.vue` | 2 min |
| F18 | `pages/jobs.list/sections/*/reads/refresh` | the live job list the dashboard gets over `GET /api/events` | the daemon's event stream is not declared in ESS (its envelope `{type: job, job}` is not an `ess-http/1` route), so no `channel` can read it | `refresh: 2s` polling of `GET /api/jobs` | 2 min |

Count: 18 entries; 5 of them (F14 to F18) remain with slot's own generator.

## Known findings of the UI session, not filed again

- A `chart` has no `sort` and plots rows by index, not by `x`.
- No machine-readable resolved document for a custom generator (F16).
