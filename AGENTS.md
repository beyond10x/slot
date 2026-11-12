# AGENTS.md — slot

Shared build slots for every build on a machine: `slot serve` owns one GNU-make jobserver FIFO
(one token per slot) and queues every job on it; `slot <command…>` runs a command through it and
streams the output back. Remote mode syncs the repository to a build server over ssh. The daemon
serves a live web view of every job. Design and measurements: `docs/design.md`.

Organization-wide rules live in `atlas/AGENTS.md` and the workspace `AGENTS.md`; this file adds
what is specific to slot.

## Serves

- **O6 — self-improvement, built into all of it.** Every build records what it did (queue wait,
  run time, CPU seconds, peak memory) in the job log, so contention between agent sessions is
  measured instead of guessed, and a change to slot sizes is re-measured against the same log.
- **O2 — decisions as data, with evidence.** One pool decides who builds when; the decision and
  its evidence (tokens held, wait, exit) are data, not per-session `flock` files and `-j` flags.

## Ownership

| path | what |
|---|---|
| `crates/slot` | library: jobserver pool, toolchains, daemon, client, `/proc` sampling, CLI definition |
| `crates/slot-cli` | the `slot` binary |
| `crates/slot-docs` | generators: CLI reference, UI TypeScript types, site provenance |
| `ui/` | web UI, Vue 3 + TypeScript + Vite + pnpm; built into `ui/dist`, embedded at compile time |
| `website/` | public documentation site, Docusaurus on `@beyond10x/docs-system` |
| `plugin/`, `.claude-plugin/marketplace.json` | the `slot` agent plugin (skills, hook, slot-warden agent) and its marketplace; plugin version = workspace version |

## Boundary

- slot schedules and observes builds; it does not cache build output (sccache does) and does not
  replace a build system.
- Adding a toolchain is one type implementing `Toolchain` plus one line in `toolchain::registry`;
  checklist in `docs/design.md` § 8.4.
- The daemon never forwards client values of `MAKEFLAGS`, `MFLAGS`, `CARGO_MAKEFLAGS` or
  `CARGO_BUILD_JOBS`: they would bypass the pool.
- Remote mode forwards only variables a toolchain's `client_env` admits, plus names listed in
  `SLOT_ENV`. Credentials never cross.
- After a normal exit the daemon does not kill the job's process group: a sccache server started
  by that job lives in it. On cancel the group is killed.

## ESS

Opted out of the workspace rule that every product repository is specified in ESS: slot is build
tooling, the class the rule exempts ("Atlas, docs and release tooling are tooling"). Revisit if
slot grows a multi-tenant service surface.

## Gate

`task check` is the complete offline gate: `cargo fmt --check`, `cargo clippy -D warnings`,
`cargo test`, the UI's typecheck, tests and build, the UX gates (`task ui-ux`), and the drift
checks of the generated CLI reference and UI types. CI runs it in `.github/workflows/gate.yml`.

## UX gates and rounds

`ui/ux/` holds the UI iteration loop. Every merge: `ui/ux/gates.spec.ts` (no sideways page scroll,
no scrolling element but the output text, the `data-ux-region` header marks fully in view,
axe-core 0 serious or critical) and `ui/ux/screen-only.spec.ts` (every scenario task done by
pointer and by keyboard, at most 2 actions, no `false_complete`) at 1366x768 and 1280x720, against
the built `ui/dist` with every API answered from `ui/ux/scenarios.ts`; then `node ux/ratchet.ts`
against `ui/ux/baseline.json`. Playwright's Chromium headless shell must be installed
(`pnpm exec playwright install --only-shell chromium` in `ui/`), and the DejaVu Sans font: the
harness renders every page in it, so the gates measure the same layout on every host.

- A change that improves a metric runs `node ux/ratchet.ts --update` and commits the new baseline
  in the same change. A baseline made worse names a decision id in that entry (`decision`).
- A round: `task ux-round -- R-nn` measures every size and writes `ui/ux/rounds/R-nn/report.md`;
  then `triage.md` (priority = severity × frequency × confidence ÷ cost, at most 5 changes) and,
  for one layout or interaction change, `decision.md`. Every finding is a
  `ui/ux/findings/UXF-nnn.md`; a closed S1 or S2 names its regression asset
  (`ui/ux/findings.test.ts`). The change set is one task under the standing UX story in `.engineering/planning/story/`.
- A new UI state or task gets a scenario in `ui/ux/scenarios.ts` before the change that adds it.

Tests use `tempfile` directories, never a fixed path under `/tmp`.

## Builds and worktrees

Change this repository in a managed worktree (`worktree create --repo ../slot --purpose …`).
Build with `CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_INCREMENTAL=0`, and finish the tree
with `worktree finish --discard-cache --archive` when the change is merged.

## Upgrading a running daemon

Install the new binary over the old path, then `systemctl --user reload slot`. Reload sends
SIGHUP. From 0.11.0 the daemon re-executes at once and hands running and queued jobs, their
slots, output pipes and client sockets to the new image (`server/handover.rs`, design.md
§ 15.2); the state file carries `version`, and a release must read every version an older one
writes. The integration test is `crates/slot-cli/tests/handover.rs`. `server/handover.rs` holds
the only `unsafe` code (adopting inherited descriptors by number); the workspace denies it
everywhere else. When the new binary is older than 0.11.0, or with `--handover false`, the
bounded hold below applies: the daemon re-executes the new binary, same pid, at the first moment no job runs. It holds
admission for at most `--upgrade-hold` (60 s) to get there; if a job still runs, admission opens
again and the next hold starts when every running job's predicted end (p90 of the same repo and
argv) is within the hold, or the pool is idle (`server/upgrade.rs`). `slot status` shows
`upgrade pending: …` until then. Queued jobs are dropped with their connections; the
client (0.3.1 and later) sees the connection close before a `start` frame, waits up to 10 min for
the socket and resubmits the same request once. A job that had started is never resubmitted, nor
one whose client got SIGINT, SIGTERM or SIGHUP: the signal thread records the signal before it
closes the connection (`client::handle_signals`).
`systemctl --user restart slot` instead drains: the socket disappears, new `slot` calls run
locally outside the pool, and the stop waits for the longest running job. Use it only when the
unit file itself changed.

Every field added to a type the daemon sends (`State`, `Tick`, `Job`, the census types, `Frame`
variants) carries `#[serde(default)]`, so a newer client reads an older daemon and the reverse.
An unknown `/api/` path answers 404, so a client can tell an older daemon from a failed call.

## Delivery

Every commit, push, tag and GitHub write is `b10x-bot[bot]` through `b10x-gates bot --repository
beyond10x/slot` and `b10x-gates api`. Never push, tag or write to GitHub as a person.

`main` takes changes only through a pull request whose required checks passed
(`common / Security and privacy`, `gate`, `Build documentation`); a direct push is refused.

1. Push a branch as the bot and open the PR with `b10x-gates gh --repo . --repository
   beyond10x/slot -- pr create --base main --head <branch> --title … --body-file <file>`.
2. `gh pr checks <n> -R beyond10x/slot --required --watch`.
3. Merge as the bot with a merge commit through the API: `{"merge_method":"merge","sha":"<head>"}`
   to `PUT /repos/beyond10x/slot/pulls/<n>/merge` with `b10x-gates api`. `gh pr merge` refuses
   (it reads the merge state without the bot's bypass), and a rebase merge makes GitHub the
   committer, which the identity ruleset rejects.
4. Tags go on the merge commit on `main`, pushed as the bot.

## Release

Bare SemVer tags (`0.1.0`).

1. Bump `version` in `[workspace.package]` of `Cargo.toml`, let `cargo build` update
   `Cargo.lock`, and move the `CHANGELOG.md` entries under `## <version> — <YYYY-MM-DD>`.
   Update the docs in the same commit: `version=` in `website/docs/getting-started.md`, `version` in
   `plugin/.claude-plugin/plugin.json` and `plugin/.codex-plugin/plugin.json`
   (`crates/slot-docs/tests/docs_version.rs` fails otherwise), and every page that describes
   what the release changes (`toolchains.md`, `reference/environment.md`, `agent-integration.md`,
   `task generate` for the CLI reference). `task check` green on that commit.
2. Annotated tag on `main`, created and pushed as the bot.
3. The tag push runs `.github/workflows/release.yml` (contents: read): `task check`; tag,
   `Cargo.toml` and `CHANGELOG.md` agree; two Linux targets built and smoke-tested (x86_64 and
   aarch64 gnu; slot reads `/proc` and has no macOS port). It uploads the artifact
   `slot-<version>-release`: `slot-<version>-<target>.tar.gz` for both targets, `SHA256SUMS`
   and `notes.md` (the changelog section).
4. `gh run download <run> -n slot-<version>-release -D target/release-assets` (read-only), then
   `sha256sum -c SHA256SUMS` there.
5. From the checkout, with paths relative to it (Gates refuses a home-directory path):
   `b10x-gates gh --repository beyond10x/slot -- release create <version> --verify-tag --title
   "slot <version>" --notes-file target/release-assets/notes.md target/release-assets/slot-*.tar.gz
   target/release-assets/SHA256SUMS`.
6. Report released only after the release author is `b10x-bot[bot]` and the downloaded assets
   pass `sha256sum -c`.

## Never

- Never put private names, home paths, host names or planning ids in source, a page, a crate
  `description` or a commit: the repository is public.
- Never commit `ui/dist/`, `ui/node_modules/` or `website/build/`.
- Never hand-edit `ui/src/generated/types.ts` or `website/docs/reference/cli.md`: run
  `task generate`.
