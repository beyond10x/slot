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
| `spec/` | ESS specification of the compute cluster (§ ESS) |
| `plugin/`, `.claude-plugin/marketplace.json` | the `slot` agent plugin (skills, hook, slot-warden agent) and its marketplace; plugin version = workspace version |

## Boundary

- slot schedules and observes builds; it does not cache build output (sccache does) and does not
  replace a build system.
- Adding a toolchain is one type implementing `Toolchain` plus one line in `toolchain::registry`;
  checklist in `docs/design.md` § 8.4.
- The daemon never forwards the client's values of `MAKEFLAGS`, `MFLAGS`, `CARGO_MAKEFLAGS` or
  `CARGO_BUILD_JOBS`: they would bypass the pool. With `slot job-parallel` on it sets
  `CARGO_BUILD_JOBS` and `MAKEFLAGS=-j` itself and leaves the pool's jobserver out of the job.
- Remote mode forwards only variables a toolchain's `client_env` admits, plus names listed in
  `SLOT_ENV`. Credentials never cross: a name that looks like one stays even when listed
  (`toolchain::credential`).
- Remote mode's safety rules live in the binary, not in a wrapper: the never-sync list
  (`never_sync.rs`), the credential names, and symlinks sent as links (`sync::preflight`). The
  list's content is host configuration; never write a listed path into source, a test or a doc.
- After a normal exit the daemon does not kill the job's process group: a sccache server started
  by that job lives in it. On cancel the group is killed.

## ESS

The compute cluster is specified in ESS before it is built: the operator decided on 2026-11-23
that slot is spec first for the cluster of build nodes (several nodes behind the local daemon).
The specification is `spec/` (`spec/ess-inputs.yaml` lists its files): `slot.cluster` (cluster,
node, placement of a worktree, the node a job ran on), `slot.hostconfig` (the host's
`cluster.toml`, outside every repository) and `slot.provisioning` (the provider contract). Check
it with `ess specify validate --path spec`; a story of the cluster cites the declaration it
implements. The design is `docs/design/remote-pool.md`; the plan is `epic:compute-cluster`.

The rest of slot keeps its opt-out of the workspace rule that every product repository is
specified in ESS: it is build tooling, the class the rule exempts ("Atlas, docs and release
tooling are tooling").

## Gate

`task check` is the complete offline gate: `cargo fmt --check`, `cargo clippy -D warnings`,
`cargo test`, the UI's typecheck, tests and build, the UX gates (`task ui-ux`), the docs site
build and its UX gates (`task site`, `task site-ux`), and the drift checks of the generated CLI
reference and UI types. It prints `rustc --version` first: `rust-toolchain.toml` pins the one
rustc of the local gate and of every workflow, which install the version that file names.
CI runs it in `.github/workflows/gate.yml`.

Its shell tests (`contrib/slot-shim.test.sh`, `contrib/rustc-gate.test.sh`,
`contrib/node/test.sh`) run once each through `contrib/shell-tests.sh`: under dash where dash is
on PATH, as on the forge, whose `sh` is dash, else under `sh`. Its first line says which
(`shell tests: under dash (…)` or `shell tests: under sh (…): no dash on PATH`). A test that
fails only under dash is a bashism in the test or in the script it tests.

A pull request whose changed files are all planning or prose files (everything under
`.engineering/`, `*.md` outside `website/` and `ui/`) passes `gate` and `Build documentation`
without building (`website/docs/getting-started.md` counts as code: `cargo test` reads it):
the step after the checkout, `.github/scripts/pr-scope.sh`, prints the paths
it saw and decides; its `case` block is the one path list. A pull request that changes only
`website/` (and such files) runs the docs checks (`task site`, `task site-ux`, the
documentation build) and not the code's build and tests. A push to `main` builds everything.
Test: `sh .github/scripts/pr-scope.test.sh`, also run by `cargo test`.

## UX gates and rounds

`ui/ux/` holds the UI iteration loop. Every merge: `ui/ux/gates.spec.ts` (no sideways page scroll,
no scrolling element but the output text, the `data-ux-region` header marks fully in view,
axe-core 0 serious or critical), `ui/ux/screen-only.spec.ts` (every scenario task done by
pointer and by keyboard, at most 2 actions, no `false_complete`, a task on a job without a scroll)
and `ui/ux/findings.spec.ts` (the assertions of closed findings) at 1366x768 and 1280x720, against
the built `ui/dist` with every API answered from `ui/ux/scenarios.ts`; then `node ux/ratchet.ts`
against `ui/ux/baseline.json`. Each run serves `ui/dist` on a free port (`ui/ux/port.ts`; `UX_PORT`
sets one), so worktrees run the gates at the same time; `ui/ux/concurrent.e2e.ts` runs two at once.
Both suites serve their built files with `ui/ux/serve.ts` (the rules of `vite preview` and
`docusaurus serve`, no symlinks, exit 1 on a missing build directory) on a free port. The docs
gates load each page once per theme and measure 1366x768, then 1280x720 after a resize, so a
layout a script sets only when a page is first drawn at 1280x720 is not caught
(`website/ux/gates-adversary.e2e.ts`, run by hand, shows it).
Playwright's Chromium headless shell must be installed
(`pnpm exec playwright install --only-shell chromium` in `ui/`), and the DejaVu Sans font: the
harness renders every page in it, so the gates measure the same layout on every host.
Playwright stops a test at 60 s, in `ui/` and in `website/`, and every run ends with its ten
slowest tests (`ui/ux/slowest.ts`). `task ui-ux` and `task site-ux` each run their commands in one
shell under `node ui/ux/deadline.ts 120`. A Playwright run that reaches 119 s prints the slowest
tests and the tests it is in (the script sets `STEP_DEADLINE_AT`). At 120 s of run time the step
gets SIGINT, 3 s later everything it started is killed, the web server in its own session
included; the script exits 124 and the task fails. Pass or fail, the script's last line is the
time the step took. The commands are not a nested `task`: Task copies the output of what it runs
and drops what is printed after it left. Two things keep a test short, and a new test keeps both:

- A scenario step waits for the page, not for a fixed time: `run.step()` moves the page's timers
  1.1 s ahead (`page.clock.fastForward`), then waits until no request of the page is in flight and
  a frame changed nothing (`ui/ux/harness.ts`).
- axe-core is loaded into a page once and run in it at every state (`axeSerious` in
  `ui/ux/probes.ts` and `website/ux/probes.ts`), not loaded again per state and merged in a
  second page.

- A change that improves a metric runs `node ux/ratchet.ts --update` and commits the new baseline
  in the same change. A baseline made worse names a decision id in that entry (`decision`).
- A round: `task ux-round -- R-nn` measures every size and writes `ui/ux/rounds/R-nn/report.md`.
  It exits non-zero when a result file is missing (a test that crashes writes none; the report
  lists each and counts it as a gate problem or a screen-only failure) or when the round ratchet
  fails (`node ux/ratchet.ts --round`, its last command; the report shows its lines). The round
  ratchet also compares the keys that depend on the host's fonts (`reading_load.*`,
  `layout_shift.total`); `--round --update` takes them in. Then
  `triage.md` (priority = severity × frequency × confidence ÷ cost, at most 5 changes) and,
  for one layout or interaction change, `decision.md`. Every finding is a
  `ui/ux/findings/UXF-nnn.md`; a closed S1 or S2 names its regression asset
  (`ui/ux/findings.test.ts`). The change set is one task under the standing UX story in `.engineering/planning/story/`.
- A new UI state or task gets a scenario in `ui/ux/scenarios.ts` before the change that adds it.

`website/ux/` holds the same loop for the docs site, against the built `website/build`
(`task site-ux`, part of `task check`): every sitemap page at 1366x768 and 1280x720 in both themes
with no sideways page scroll, nothing past its box, navbar, search, theme toggle and sidebar in
view at the top and the bottom, axe-core 0 serious or critical (`gates.spec.ts`); the reader
tasks in `tasks.ts` done from the landing page by pointer and by keyboard in at most 3 actions
(`screen-only.spec.ts`); `ratchet.ts` against `website/ux/baseline.json`, reusing
`ui/ux/ratchet-lib.ts` (keyboard Tab counts and layout shift only in a round); `copy.test.ts` (the landing page names no version but the workspace's, a
`planned` item in `website/data/status.json` points at a "Not built yet" section that still
lists it). A round: `task site-ux-round -- D-nn`; findings are `website/ux/findings/DXF-nnn.md`.
A page or a feature that a reader should find gets a task in `website/ux/tasks.ts`.

A docs test waits for the page, not for a fixed time (`website/ux/settle.ts`; `budget.test.ts`
fails on a new `waitForTimeout`): `openPage` until the page is hydrated and at rest (no script or
request in flight, no scroll or transition running, a frame that changed nothing), the reader
after a scroll until `scrollend`, after a link until the other page is drawn. The reader's search
and menu paths keep their three fixed waits: no task takes them, so a wait on a condition there
was never run. The reader sends its Tab presses 25 at a time; the page counts them and drops those
after the target. It waits on the search box until the index is loaded, because the box takes the
focus back then (DXF-010); with that its Tab counts were the same in two runs.

Tests use `tempfile` directories, never a fixed path under `/tmp`.

## Builds and worktrees

Change this repository in a managed worktree (`worktree create --repo ../slot --purpose …`).
Run `git fetch origin` first and pass `--base origin/main`: the default base is the local
`HEAD`, and a local `main` that was not pulled gives a tree many releases old (a UX round
and a review were once started on 0.10.0 while `origin/main` was 0.19.0).
Pass `--seed-target` (`worktree` 0.21.0 and later; `WORKTREE_SEED_TARGET=1` does the same): it
copies the dependency artifacts of the repository's newest built checkout into the new tree's
`target/`, so the first `cargo` call there does not compile every dependency again.
Build with `CARGO_INCREMENTAL=0` and the host's `debug` setting (no `CARGO_PROFILE_DEV_DEBUG`),
and finish the tree
with `worktree finish --discard-cache --archive` when the change is merged.

## Upgrading a running daemon

Before the install, read `slot --version` of the installed binary and `git worktree list`: a
version with a `+` suffix is a local build of another session. The release that replaces it must
contain its changes, and both must write the same handover `VERSION` (`server/handover.rs`).
After the reload, compare the job ids of `/api/state` before and after.

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
`systemctl --user restart slot` instead drains: the socket disappears, new `slot` calls wait
60 s for it and then exit 125 without running (0.25.2; before, they ran outside the pool), and the
stop waits for the longest running job. Use it only when the
unit file itself changed.

Every field added to a type the daemon sends (`State`, `Tick`, `Job`, the census types, `Frame`
variants) carries `#[serde(default)]`, so a newer client reads an older daemon and the reverse.
An unknown `/api/` path answers 404, so a client can tell an older daemon from a failed call.

## Delivery

Every commit, push, tag and GitHub write is `b10x-bot[bot]` through `b10x-gates bot --repository
beyond10x/slot` and `b10x-gates api`. Never push, tag or write to GitHub as a person.
`b10x-gates gh` and `b10x-gates api` need `B10X_GATES_POLICY` set to the gates policy file;
without it they stop with `protected file … unavailable`. A plain `git commit` is refused by a
hook: commit with `b10x-gates bot --repository beyond10x/slot -- commit -F <file>`.

Standing exception: agents working on slot commit, tag and release without asking each time,
under the operator's standing instruction of 2026-11-12 ("from time to time you issue some
commits of your own code and cut releases"; "always update your documentation when you make a
release"). Every commit still goes through the bot and a pull request below.

Merges are the operator's: a session sends the operator one line,
`beyond10x/slot#<n> <full head sha>`, when the checks are green, and does not merge itself; the
operator merges after its own scan of the diff. A change that widens what the node accepts
(`slot shell`, a copy path to a node, a deny list) gets a read-only review first.

`main` takes changes only through a pull request whose required checks passed
(`common / Security and privacy`, `gate`, `Build documentation`); a direct push is refused.

1. Push a branch as the bot and open the PR with `b10x-gates gh --repo . --repository
   beyond10x/slot -- pr create --base main --head <branch> --title … --body-file <file>`.
2. `gh pr checks <n> -R beyond10x/slot --required --watch`, run in the background: the session
   must not block on it.
3. Send the PR line to the operator, who merges (above). How the merge is made, for
   reference: as the bot with a merge commit through the API:
   `{"merge_method":"merge","sha":"<head>"}`
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
   `task generate` for the CLI reference), and the landing page: a shipped feature gets a line in
   `website/data/status.json` (and a planned one turns `shipped`), its `source` range ends at the
   new version (`website/ux/copy.test.ts` fails otherwise), a reader-facing one a card in
   `website/product.json`. `task check` green on that commit.
2. Annotated tag on `main`, created and pushed as the bot.
3. The tag push runs `.github/workflows/release.yml` (contents: read): `task check`; tag,
   `Cargo.toml` and `CHANGELOG.md` agree; four Linux targets built and smoke-tested, each
   architecture on a runner of its own kind (`x86_64-unknown-linux-gnu`,
   `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`; slot
   reads `/proc` and has no macOS port). A musl binary is also proven static
   (`.github/scripts/static-binary.sh`: no `NEEDED` entry, no program interpreter) and answers
   `--version` in an Alpine container whose image the workflow names by digest. It uploads the
   artifact `slot-<version>-release`: `slot-<version>-<target>.tar.gz` for the four targets,
   `SHA256SUMS` and `notes.md` (the changelog section). `crates/slot-docs/tests/ci/mod.rs` reads
   the workflow: a target added to the matrix is added to the bundle's list, to
   `RELEASE_TARGETS` there and to `website/docs/getting-started.md`.
4. `gh run download <run> -n slot-<version>-release -D target/release-assets` (read-only), then
   `sha256sum -c SHA256SUMS` there.
5. From the checkout, with paths relative to it (Gates refuses a home-directory path):
   `b10x-gates gh --repository beyond10x/slot -- release create <version> --verify-tag --title
   "slot <version>" --notes-file target/release-assets/notes.md target/release-assets/slot-*.tar.gz
   target/release-assets/SHA256SUMS`.
6. Report released only after the release author is `b10x-bot[bot]` and the downloaded assets
   pass `sha256sum -c`.

### Concurrent releases

Several agents may prepare releases at once. Version numbers are not reserved in advance.

- A release PR takes the next minor (or patch) after the newest tag on `origin/main` only when it
  is next in the merge queue and ready to merge.
- The PR behind it waits for that merge, then soft-resets onto `origin/main`, bumps
  `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`, `version=` in `website/docs/getting-started.md`
  and the plugin manifests, and re-commits as the bot.
- One agent owns the live reload at a time. It reloads only after the previous release is
  installed, never while another reload is pending.

## Live daemon

Change the live daemon only through `slot` commands, `systemctl --user reload slot` and the HTTP
API. Writing tokens into the live jobserver FIFO by hand is a last resort for a verified leak
(every FIFO holder listed from `/proc/*/fd`, tokens counted), reported with the numbers.

## Open work

Done: 0.1.0 to 0.30.0 (`CHANGELOG.md`): one jobserver pool with a queue ordered by job priority,
passing within a level for short jobs and whole-pool requests, admission by disk and memory,
holds on the host's CPUs, admission by share (off by default), run-time limit, build processes
per job, headroom and call mode, headroom of the machine, slots named by the job (`--tokens`),
the web view with each project's share of the pool (`slot usage`), remote mode, the gate for
builds in a sandbox, handover on reload, lost-token recovery, and eviction of local target
directories (deletes by default, `evict.json` sets a dry run).

Open, one line each, with where the design lives. A session starts with (e).

- (a) The rest of the conductor contract (job priority is built; labels, version endpoint, OpenAPI,
  strict mode, run report, write token, prompt-prefix hook): the conductor repository,
  `docs/design/slot-integration.md`.
- (c) A warning for mixed `CARGO_PROFILE_*` in one target directory: `docs/design.md` § 12,
  idea 2, not built; a job's `cache_env` in the job log has the values to compare. The warning
  for an empty `RUSTC_WRAPPER` (idea 1) is built, without asking whether sccache is configured.
- (d) I/O and disk and cache sizes in the live view: about ½ to 1 day each; `io.stat` needs a
  root `Delegate=` drop-in for the user manager.
- (e) The live daemon may still run an old binary: compare
  `ls -l /proc/$(systemctl --user show slot -p MainPID --value)/exe` and `slot status` with the
  newest release; reload only as § Upgrading a running daemon says.
- (f) Headroom base `machine` (`slot headroom --base machine`): its `set-property` path has
  not run against a live `systemctl` in a test; read `slot status` after switching a host to it.
- (g) A reload re-execs the new binary with the old process's own arguments (`std::env::args()`
  in `server/mod.rs`), so a changed `ExecStart` option such as `--min-free-disk` takes effect only
  at a full restart. Options that should change on reload belong in `limits.json`, like the
  headroom, or in `priority.json`, like the promotion delay (`slot priority promote-after`).
- (h) A reload sent right after a handover can drop the queued jobs: the second handover carried
  20 running and 0 queued jobs where the first carried 20 queued (1 of 10 runs beside 20 busy
  loops). `resume` re-queues handed-over jobs in tasks through `run_job`, and a reload before
  they run hands over their connections without the jobs; the clients resubmit them with a new
  id at the back of the queue (inferred from `client.rs`):
  `story:a-quick-second-reload-keeps-queued-jobs`.
- (i) Admission by share (`slot admission shares`, built, off by default) admits by CPU share and
  lends the share of idle jobs, answering (v) and (z). Its budget rule fell to 1 CPU when
  replayed on this host's job log until it got a lower bound; it is measured with shares on, on
  one host, before it is switched on, and the switch is the operator's (design.md § 26).
- (l) `task ui-ux` and `task site-ux` stop themselves at 120 s and print their ten slowest
  tests. On this workstation both still stop at 120 s at a load of 50 to 170: `site-ux` with 23
  of 62 tests done at load 57, `ui-ux` with about 55 of 93 at load 57 to 78. The cause measured
  is the pool's CPU weight (`slot serve --cpu-weight`, 20 by default, against 100 for
  interactive work): the docs gate tests took 54 s in a cgroup of their own and 5.1 minutes in
  the pool at load 98 to 140. The docs gates load each page once per theme (97 to 65
  CPU-seconds). Left: three forge runs of `ui-ux` under 100 s, and a decision on the CPU weight
  of gate steps: `story:ux-gate-step-ends-within-its-budget`.
- (o) Which Rust units are compiled again, and how often, is recorded nowhere: on 2026-11-22
  sccache answered 26 % of rustc calls from the cache and refused 57 %, and 35 % of the units on
  disk repeat one built in another target directory:
  `story:jobs-record-what-the-compile-cache-did`.
- (p) A job runs in the daemon's cgroup (`slot.service`), not the caller's: a test that needs
  its caller's delegated scope passes run directly and fails through the shim. `slot --local`
  and `SLOT_SHIM_OFF=1` keep the caller's cgroup and leave the pool:
  `story:a-job-can-run-in-the-callers-cgroup`.
- (q) The round keys of `ui/ux/baseline.json` (`reading_load.*`, `layout_shift.total`) predate
  the header of 80 px: `node ux/ratchet.ts --round` fails until a round takes them in, and a
  `reading_load` made worse names a decision id.
- (r) `cpu_s` of a cancelled job is read too low: of 22 cancelled jobs that ran over 5 minutes, 14
  show under 0.05 CPUs per second of run time (2026-11-22; 4 of 207 among jobs not cancelled). A
  cancelled run that computed looks like one that waited.
- (s) A build that waits for its target directory holds back only cargo jobs:
  a script job (`task check`) records no target directory, so its inner cargo still blocks on
  the lock of a running build.
- (t) The web view has no dark theme.
- (u) The pool size (`slot serve --tokens`) changes only at a full restart, and a restart drains:
  the old daemon runs every running and queued job to its end under the old size, while new
  calls find no socket and, from 0.25.2, exit 125 after 60 s without running (before, they ran
  outside the pool, unrecorded). Measured with 69 jobs: about 4 ended
  a minute. The pool size belongs in `limits.json` beside the headroom, changed by a command.
- (v) Script jobs hold slots they do not use: over 3 hours, 256 jobs whose command was a script
  held 60 % of the slot-hours at 0.74 CPUs per slot (plain `cargo` jobs: 1.47), and 63 of them
  ran over 5 minutes. One sample had every slot of 17 taken, 59 jobs queued and the pool at
  7.8 CPUs. The same cause as (i). 100 of the 256 named no repository, so no priority rule and
  no usage row matched them.
- (w) After `slot: job N queued` the client prints nothing until the job starts, so a shell that
  waits 30 minutes looks hung. It should print the place in the queue from time to time.
- (x) Priority rules that put the busiest repositories on `high` make every other job wait for
  the promotion delay: in 40 minutes 55 of 62 `normal` jobs started only promoted, median wait
  1,841 s, while `high` jobs waited 204 s. The delay is a setting now
  (`slot priority promote-after`); the default is still 1800 s, and levels are strict, not shares.
- (y) The usage table labels a row `unverified` without saying what it means: the job named no
  checkout and its project was not read from a git remote, so the name is the directory's.
- (z) The default pool size is the number of CPUs, on the assumption that a slot keeps one CPU
  busy. Measured on a host of 20 CPUs: 0.90 CPUs per slot over 3 hours, a quarter of the
  slot-hours in jobs under 0.25 CPUs per slot, and the compilers run in the compile cache server,
  outside the job. With 17 running slots 15.2 CPUs were busy and 59 jobs queued; with 50 running
  slots (`--tokens 59`, headroom 15 %, the same CPU quota) 20.0 were busy and the longest wait
  fell from 31 minutes to under 5; in 15 minutes 12.3 jobs a minute ended against 7.7, the 90th
  percentile of the queue wait was 97 s against 1,467 s, and single jobs ran longer (4 commands
  seen in both windows, `pnpm build` 8 s then 120 s).
  Design, the operator's, built off by default as admission by share (i): no fixed count.
  The daemon reads the CPUs of the host and what the pool uses, and admits the head of the
  queue while a share of CPU is free for it (about
  0.4 CPUs per job); below that it admits nothing. A lower and an upper bound on the number of
  running jobs hold both ends. `policy::Controller` already computes a pool size from the census
  every tick and only shows it (`recommended_tokens`); (i) and (v) are cases of the same change.
  A job then asks for a share of the CPUs, not for a number of slots:
  `story:jobs-ask-for-a-share-of-the-cpus`.
- (aa) The CPU limit for starts (`slot cpu-limit`) counts the pool and the compile cache server
  only (0.25.0). On the live host since 2026-11-23, set by the operator. A watch that switches
  it off on the queue wait must count the ages of the queued jobs: the waits of started jobs
  alone read 46 s while 39 queued jobs had a median age of 200 s (2026-11-22). The CPU-time
  figure cannot see demand once the host is saturated (load 208 on 20 CPUs with the pool at
  11.5 CPUs under a mark of 12, 2026-11-23). 0.27.0 adds a pressure mark
  (`slot cpu-limit <mark> --pressure P`, off by default). Measured on the live host, 30 minutes
  with `--pressure 80` against 30 minutes without (2026-11-23): 155 jobs against 176, queue wait
  p90 1,264 s against 996 s, peak load 132 against 136. It held more and lowered nothing, so it
  was switched off again. CPU pressure there was 74 % at the median and 80 % at p75 over 3 hours:
  it saturates near 80 % at a load of 100 as the CPU figure does, and a mark under that holds
  most of the day. Every held job still starts on the 10-minute bound (40 in that window), so
  the wait is set by the bound and the queue's length, not by the mark.
- (ab) `--min-free-disk` is an option of `slot serve`, so a change needs a full restart (g); it
  belongs in `limits.json`. The eviction mark is there already (`slot evict-below`).
- (ac) The compute cluster: several build nodes behind the local daemon, specified in `spec/`,
  designed in `docs/design/remote-pool.md`, planned in `epic:compute-cluster`.
- (ad) A node's `price_per_hour_cents` in `cluster.toml` is typed by hand; after a change of
  instance size it was halved, not read (2026-11-24):
  `story:a-nodes-price-is-read-from-the-provider`. The resize procedure is in
  `contrib/node/README.md` § A change of instance size.

## Never

- Never put private names, home paths, host names or planning ids in source, a page, a crate
  `description` or a commit: the repository is public.
- Never run an `aep` command that writes without `AEP_ACTOR=agent:slot` in its environment.
  Without it the store records `human:<login>` in each transition and evidence record. The 200
  evidence files written before this rule still carry it: evidence is never rewritten, and
  `aep plan artifact redact` does not reach them.
- Never commit `ui/dist/`, `ui/node_modules/` or `website/build/`.
- Never hand-edit `ui/src/generated/types.ts` or `website/docs/reference/cli.md`: run
  `task generate`.
