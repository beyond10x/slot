---
name: building
description: Run builds, tests and lints through the shared slot pool so parallel agent sessions do not oversubscribe the machine — prefix the command with `slot`, check the daemon, read queue wait in the live view, use `--local` or a build server. Use when `slot` is on `PATH` and a session is about to run a compiling or test command (`cargo build`, `cargo test`, `cargo clippy`, `go test`, `make`, `task check`, `npx vitest`, `npx playwright test`), when a Bash command was refused with a line starting `slot:`, when a build seems to hang with no output, when the user asks why builds are slow or queued, which sessions build outside the pool, or mentions slot, `SLOT_HOST`, the headroom or the slot web view. Not for installing slot, which is `slot:init`.
---

# Building through slot

`slot <command…>` hands the command to the slot daemon, which queues it until enough slots are
free, runs it in the same directory with the same environment, streams its output back and exits
with its exit code. Every build that goes through the daemon draws from one GNU-make jobserver, so
cargo, rustc and make in all sessions share one pool of slots and the machine is never
oversubscribed.

## Before the first build of a session

Run `command -v slot`. Nothing: build without the prefix, and name `slot:init` if the user wants
the shared pool. Then check the daemon:

```bash
curl -fsS http://127.0.0.1:7411/api/state
```

JSON with `tokens` (pool size), `free`, `active` and `done` means it runs. If it does not answer,
prefixing is still safe: `slot` prints `slot: daemon unreachable (…), ran locally` on stderr and
runs the command here without the pool. Tell the user once, and do not start a daemon from the
session; `slot:init` sets one up.

## Which commands get the prefix

Prefix every command that compiles or runs tests, at the outermost level: `slot task check`, not
a `task check` whose recipe calls `slot cargo …`. A `slot` call inside a job that already runs
through slot (`SLOT_JOB` is set) runs its command directly on the outer job's slots and queues
nothing, so a script or Taskfile that prefixes its own builds is safe to run through slot too.

| command | how it shares the pool |
|---|---|
| `slot cargo test -p foo`, `slot cargo build`, `slot cargo clippy` | cargo and rustc draw their parallelism from the pool |
| `slot go test ./...` | holds a fixed share (4 slots by default), and go's `-p` is set to match |
| `slot task check`, `slot make` | the wrapper holds one slot and passes the pool down; cargo and make inside share it |
| `slot yarn vitest run --maxWorkers=4`, `slot npx playwright test --workers=2` | holds one slot per worker; always pass the worker flag, since without it the runner starts about one worker per core while holding 4 slots |
| `slot ignite test a.ignite --jobs 4`, `slot ignite build a.ignite` | test and conformance hold one slot per `--jobs` (4 without the flag); build holds one, and the Rust target's cargo shares the pool |
| `slot pnpm build`, `slot npx tsc --noEmit`, `slot scripts/zig.sh build`, `slot pytest -q` | holds one slot |
| anything else | holds one slot |
| `slot --tokens 4 ./build.sh` for a script that starts `docker run --cpus 4` | holds the 4 slots named, capped at the slots in circulation; use it for any command whose CPUs the daemon cannot see |

Test binaries in rust, go and wrapper jobs get `RUST_TEST_THREADS`, `NEXTEST_TEST_THREADS` and
`GOMAXPROCS` from the daemon (a quarter of the pool; go jobs their slot count). Set them yourself
only when a suite needs a specific value; yours is kept.

Leave out commands that do not build: `git`, `cargo fmt`, `cargo metadata`, editors and
rust-analyzer. When the command's own name is a slot subcommand (`serve`, `proxy`, `run`,
`status`, `headroom`, `sessions`, `hook`), use `slot run <command…>`.

The job's standard input is empty: an interactive command (a prompt, a REPL) does not work through
slot. Interrupting `slot` (Ctrl-C, a tool timeout, killing it) cancels the job, kills its process
group and returns its slots.

## Waiting and reading the queue

A job that has to wait prints one line on stderr before it starts:
`slot: job N queued for K slot(s): F free, A request(s) ahead`. Until its output begins it is
waiting, not hung. Queue wait
adds to the wall time: give a build through slot the same generous timeout as a long build, or run
it in the background. To see where it stands, run `slot status` (the pool and every queued and
running job, one line each), open <http://127.0.0.1:7411/> (live timeline, slot usage, cores per
toolchain, each job's output), or read the state:

```bash
curl -fsS http://127.0.0.1:7411/api/state
```

Each job in `active` has `state` (`queued` or `running`), `argv`, `repo`, `tokens` (slots held),
`enqueued_ms` and `started_ms`; wait is `started_ms - enqueued_ms`, and a queued job has
`started_ms` 0. Finished jobs in `done` also carry `exit`, `cpu_s` and `peak_rss`. Report a long
wait with the jobs ahead of it; do not work around it.

## A refused command

In Claude Code the plugin's `PreToolUse` hook refuses a Bash command that compiles or runs tests
outside the pool, with one line on stderr:

```text
slot: `npx vitest run` compiles or runs tests outside the shared build pool. Run `slot npx vitest run --maxWorkers=4` instead (the job holds one slot per worker, so name the worker count).
```

Run the command it names. It refuses `cargo` build, check, test, clippy, doc, bench, nextest and
install subcommands and `go build`, `test`, `vet`, `install` (unless the PATH shim already sends
them through slot), `make -j…`, `make` and `task` targets named like a build (`check`, `test`,
`build`, `lint`, `gate`, `verify`, `generate`, `all`; not `deploy`, `dev`, `up`, `watch`), `npm test`, `yarn test`, `pnpm
test`, and vitest, jest and `playwright test` through npx, bunx, yarn, pnpm or `node_modules/.bin`;
`tsc`, `vue-tsc`, `eslint`, and `vite`/`docusaurus`/`webpack`/`next` `build`; package.json scripts
named like a build (`pnpm build`, `yarn typecheck`, `npm run test:unit`, not `dev` or `*:watch`);
`node --test` and `node <script> gate|test|check|…`; `zig build|test` (also `scripts/zig.sh`);
`pytest` and `python -m pytest`; `mvn`/`gradle` goals that compile or test; `ignite
build|test|conformance` unless the PATH shim covers it.
It looks into `bash -c '…'` and every part of `a && b; c | d`, and ignores quoted text, comments
and heredoc bodies. `slot …`, `SLOT_SHIM_OFF=1 …` and every command inside a slot job (`SLOT_JOB`
set) pass. The user sets `SLOT_HOOK=warn` (advice as context, nothing refused) or `SLOT_HOOK=off`;
a session never sets it to get around a refusal.

## The warden

`slot sessions` lists the agent sessions that build outside the pool: cores outside slot averaged
over the last 60 s of the daemon's history, and the commands running outside it now, by pid.
`/slot:warden` starts the slot-warden role on it. When the host exposes subagents, delegate to the
`slot-warden` agent; otherwise run [references/slot-warden.md](references/slot-warden.md)
directly. It reports, and in nudge mode asks each session to rerun through slot; it never kills a
process or edits another session's files.

## `--local`

`slot --local <command…>` runs the command here without the daemon and without the pool. Use it
only for what the pool cannot run: a test that needs a local service in remote mode (below), or to
check whether a failure comes from slot itself. Never use it to skip the queue.

## Remote mode

When `SLOT_HOST` names a build server, `slot` syncs the repository to it with `rsync` (without
`.gitignore`d files, `.git` and `target/`; uncommitted changes are included), runs the command
there through `ssh <host> slot proxy` and rewrites the server's paths in the output to the local
ones. If the server cannot be reached, the command runs locally and `slot` says so in one line.

- Only the toolchain's own variables (for cargo: `CARGO_*` except `CARGO_TARGET_DIR`,
  `CARGO_HOME` and `CARGO_BUILD_JOBS`, plus `RUSTFLAGS` and the like) and the names listed in
  `SLOT_ENV` (comma separated) reach the server. Credentials never do; do not add them to
  `SLOT_ENV`.
- Build output stays on the server: `slot cargo build` leaves no binary in the local `target/`.
  Build a binary you will run here with `slot --local`.
- Tests that need docker, a database, a kind cluster or a port on this machine fail on the server;
  run those with `slot --local`.

## Never

- Never set `CARGO_BUILD_JOBS`, `-j`, `--jobs`, `MAKEFLAGS` or `CARGO_MAKEFLAGS` yourself, and never
  wrap a build in a `flock` slot file: the pool replaces them, and the daemon drops those variables
  anyway.
- Never start, stop or restart the daemon as part of a build; it serves every session on the
  machine. Upgrading it is `slot:upgrade`.
- Never put credentials into `SLOT_ENV`.
