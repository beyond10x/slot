# Gaps of ESS met by slot's UI spec (ESS 0.98.0)

Story texts for the ESS repository, one per gap, from the job list and job detail of
`spec/ui/slot.ui.yaml`. The friction each caused, and what slot did instead, is in
[ess-ui-friction.md](ess-ui-friction.md) (F-numbers). G12 is already filed by the UI session and
is listed only to keep the numbers.

## G1 — a view route cannot read its rows from a member of a larger answer (ess-http/1)

- Observed: `GET /api/state` answers one object whose `active` and `done` members hold the jobs. A
  view route's `body` is `rows`, `array` or `single`, so neither view can be bound to it, and the
  generated client reads the whole object as one row. slot added two routes to its daemon (F1).
- Expected: a view route names the member its rows sit under, and two views may read one answer.
- Snippet:
  ```yaml
  - view: slot.pool.ActiveJobs
    method: get
    path: /api/state
    body: {array_at: active}
  ```

## G2 — a Binary64 literal is refused in `sets` (ess/37)

- Observed: `sets: {cpu_s: 0.0}` on a `Binary64` field is `type_mismatch`: "a literal may be
  text, … a whole number or a decimal" (F2).
- Expected: a decimal literal sets a `Binary64` field.
- Snippet: `sets: {cpu_s: 0.0}` with `fields: [{name: cpu_s, type: Binary64}]`.

## G3 — an integer field is drawn with the locale's grouping (React target)

- Observed: `{field: id, label: Job}` over an `Integer` newtype shows `61,259`. No `as` draws an
  integer as written (F5).
- Expected: an identifier is drawn as written: `as: text` on an integer, or no grouping for an
  identity field or a newtype of `Integer`.
- Snippet: `columns: [{field: id, label: job, as: text}]`.

## G4 — `as: list` joins with a comma (React target)

- Observed: `argv: List<String>` with `as: list` shows `task, ux:image` and wraps (F6).
- Expected: the field names the separator, and a cell can stay on one line.
- Snippet: `{field: argv, as: list, separator: " "}`.

## G5 — `as: map` draws a tagged union as raw JSON (React target)

- Observed: a `checkout` of a `union` type (`tag: kind`) shows `{"kind":"worktree","name":"x"}`
  (F10).
- Expected: a union reads as its variant and its fields (`worktree x`), or `labels` per variant.
- Snippet: `{field: checkout, as: map}` over `kind: union, tag: kind, variants: {worktree: …}`.

## G6 — `time` and `timestamp` are UTC only (ess-ui/1)

- Observed: the reference defines both in UTC; an operator view of a local machine shows local
  time everywhere else (F8).
- Expected: a document or field chooses the zone: `zone: local`.
- Snippet: `{field: ended_ms, as: time, zone: local}`.

## G7 — a host component stands at one place only (ess-ui/1)

- Observed: `host_slots` refuses a second slot with the same `host`, also through a YAML alias,
  so one cell component in three tables needs three names (F14, F15).
- Expected: the slot's node path identifies the slot and `host` names the component, which may
  stand in many slots.
- Snippet: three collections each with `{field: toolchain, host: JobTool, stands_in_for: text, reason: …}`.

## G8 — a search reads one field (ess-ui/1)

- Observed: `row.argv contains state.search` searches the command; there is no way to search the
  command, repository and session with one input, where any field may hold a word (F4).
- Expected: `contains` over several fields.
- Snippet: `filter: "[row.argv, row.repo, row.session] contains state.search"`.

## G9 — no column width or one-line cell (ess-ui/1, React target)

- Observed: a dense table cannot be declared: cells wrap, rows grow, a table of twelve columns runs
  past 1366 px; tokens set colours and spacing only, and no class hook lets the host style a
  column (F7, F17).
- Expected: a field declares its width and whether it wraps; the React target gives every column a
  stable class.
- Snippet: `{field: repo, width: 160px, wrap: false}`.

## G10 — no value derived from two fields and the clock (ess-ui/1)

- Observed: the wait of a job (`started_ms - enqueued_ms`, or now while it waits) and its run time
  need a host slot per column (F11).
- Expected: a field shows a duration between two fields, the clock standing in for an unset one.
- Snippet: `{name: wait, between: [enqueued_ms, started_ms], unset: now, as: duration}`.

## G11 — tones: one badge tone, five tones per value (ess-ui/1)

- Observed: `header.badge.tone` is one tone for every value (F13), and a value can only take one
  of five tones, not a colour of its own such as a toolchain's chart colour (F12).
- Expected: `header.badge.tone_by`; a colour map beside the tone maps.
- Snippet: `badge: {text: section.summary.row.state, tone_by: job_state}`.

## G12 — no machine-readable resolved document (filed by the UI session)

`ess ui load` prints a summary; a custom generator reads the YAML itself and does not apply the
page-kind merge (F16).

## G13 — a read-only entity needs invented commands (ess/37)

- Observed: an entity whose records a system keeps by itself (the daemon's jobs) is refused until
  every lifecycle transition is taken by a command outcome that emits an event
  (`missing_causation`, `empty_change`); slot declared four commands and four events that no
  HTTP caller can send (F3).
- Expected: an entity or a transition declared as recorded by its owner (`caused_by: owner`),
  with no command.
- Snippet: `lifecycle: {initial: Queued, recorded_by: owner, transitions: [...]}`.
