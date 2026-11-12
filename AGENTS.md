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
`cargo test`, the UI's typecheck, tests and build, and the drift checks of the generated CLI
reference and UI types. CI runs it in `.github/workflows/gate.yml`.

Tests use `tempfile` directories, never a fixed path under `/tmp`.

## Builds and worktrees

Change this repository in a managed worktree (`worktree create --repo ../slot --purpose …`).
Build with `CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_INCREMENTAL=0`, and finish the tree
with `worktree finish --discard-cache --archive` when the change is merged.

## Upgrading a running daemon

Install the new binary over the old path, then `systemctl --user reload slot`. Reload sends
SIGHUP: the daemon keeps taking jobs and re-executes the new binary, same pid, at the first moment
no job is active. `systemctl --user restart slot` instead drains: the socket disappears, new
`slot` calls run locally outside the pool, and the stop waits for the longest running job. Use it
only when the unit file itself changed.

## Delivery

Every commit, push, tag and GitHub write is `b10x-bot[bot]` through `b10x-gates bot --repository
beyond10x/slot` and `b10x-gates api`. Never push, tag or write to GitHub as a person.

## Release

Bare SemVer tags (`0.1.0`).

1. Bump `version` in `[workspace.package]` of `Cargo.toml`, let `cargo build` update
   `Cargo.lock`, and move the `CHANGELOG.md` entries under `## <version> — <YYYY-MM-DD>`.
   `task check` green on that commit.
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
