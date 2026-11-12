---
title: Agent integration
description: Make coding agents run their builds through slot with one line in a repository's AGENTS.md.
---

# Agent integration

slot is meant for machines where several coding-agent sessions build at the same time. An agent
needs one instruction: prefix builds with `slot`. Nothing else in its workflow changes, because
output and exit code are those of the command.

## The AGENTS.md line

Add this to the repository's `AGENTS.md` (or the agent's global instructions, to cover every
repository on the machine):

```markdown
- Run every cargo, go and task build or test through the shared build pool by prefixing it with
  `slot`: `slot cargo test -p foo`, `slot go test ./...`, `slot task check`. `slot --local <command>`
  runs without the daemon; use it for tests that need local services (docker, databases, local ports).
```

What it achieves:

- Every build from every session waits for a slot in one queue, instead of each one starting a
  compiler per core.
- The web UI and the job log show which session started which build. The client sends the
  agent's session id when it runs inside a Claude Code session (`CLAUDE_CODE_SESSION_ID`).
- If the daemon is down, builds still run locally, with one line on stderr.

## Make it the default: the PATH shim

An instruction depends on every agent following it. On a machine where every agent session should
build through slot, install the shim instead: [`contrib/slot-shim`](https://github.com/beyond10x/slot/blob/main/contrib/slot-shim),
linked as `cargo`, `go` and `ignite` in a directory that agent shells put first on `PATH`.

```sh
d=~/.local/share/slot
mkdir -p "$d/shims"
curl -fsSL -o "$d/slot-shim" https://raw.githubusercontent.com/beyond10x/slot/main/contrib/slot-shim
chmod +x "$d/slot-shim"
ln -sfn ../slot-shim "$d/shims/cargo"
ln -sfn ../slot-shim "$d/shims/go"
ln -sfn ../slot-shim "$d/shims/ignite"
```

Then put `$d/shims` first on `PATH` for agent shells only. For Claude Code, which sets
`CLAUDECODE=1` in its shells, in `~/.zshenv`:

```sh
if [[ -n $CLAUDECODE && -d $HOME/.local/share/slot/shims ]]; then
  path=($HOME/.local/share/slot/shims ${path:#$HOME/.local/share/slot/shims})
  export PATH
fi
```

A Claude Code `SessionStart` hook that appends the same `PATH` line to `$CLAUDE_ENV_FILE` does it
for every shell, bash or zsh.

What the shim sends through slot:

- `cargo build`, `b`, `check`, `c`, `test`, `t`, `clippy`, `doc`, `d`, `bench`, `nextest`,
  `install`, `rustc`, `llvm-cov`, `miri`; global flags such as `+nightly` or `--locked` before the
  subcommand are skipped when finding it.
- `go build`, `test`, `vet`, `install`.
- `ignite build`, `test`, `conformance`.

Everything else runs directly: `cargo run`, `cargo fmt`, `cargo metadata`, `go run`, `go mod`,
`ignite check`, `ignite run`. A
command inside a slot job (`SLOT_JOB` set) runs directly, so `slot task check` does not queue its
cargo calls a second time. `SLOT_SHIM_OFF=1` runs one command outside the pool. Without `slot` on
`PATH` the shim runs the real command.

## Claude Code: refuse builds outside the pool

The shim covers `cargo`, `go` and `ignite`. For everything else the
[Claude Code plugin](claude-code-plugin.md) adds a `PreToolUse` hook that refuses a build
command outside the pool and names the `slot` command to run instead, and `slot sessions` with the
`slot-warden` agent to find sessions that still build outside it. The hook refuses:

- `task` and `make` targets whose name says build, check, test, lint, gate and the like;
- vitest, jest and `playwright test`, and the TypeScript and lint tools `tsc`, `vue-tsc` and
  `eslint`; `vite`, `docusaurus`, `webpack`, `next`, `nuxt`, `astro`, `rollup` and `esbuild` with
  `build`;
- package.json scripts run through npm, yarn or pnpm whose name says build, test, typecheck,
  lint, check, e2e or gate (`pnpm build`, `npm run test:unit`), and `node --test`, or a repository
  script called with such a subcommand (`node tools/ux.mjs gate`);
- `zig build`, `zig test`, `zig build-exe|lib|obj`, also behind a repository's `zig.sh`;
- `pytest` and `python -m pytest`;
- maven and gradle goals that compile or test (`mvn package`, `./gradlew test`);
- `ignite build|test|conformance` when the `ignite` shim is not installed.

It lets a command through when it watches, serves or only prints (`--watch`, `serve`, `dev`,
`start`, `--version`). `docker build` and `helm template` are not refused:
[Toolchains outside the pool](#toolchains-outside-the-pool) says why.

## Toolchains outside the pool

Measured on one workstation shared by about 15 agent sessions, 2026-11-12 (11 h of census
samples and 10 h of agent shell commands):

- **docker and podman build** run in the container daemon, outside the job's process tree and
  cgroup, so slot could neither count nor cap them. 0.01 core-hours outside the pool in 11 h.
- **helm template and lint**: 0.05 core-hours, at most 0.6 cores at once.
- **valgrind** and other profilers measure a program's run time; queueing them changes nothing
  they measure, and a slot would only be held while they run.
- **bazel**: no call seen.

## Remove the old workarounds

Once builds go through slot, these are not needed and some are ignored:

- `CARGO_BUILD_JOBS=…` or `-j` set by hand. The daemon drops `CARGO_BUILD_JOBS`, `MAKEFLAGS`,
  `MFLAGS` and `CARGO_MAKEFLAGS` from the caller's environment.
- `flock` slot files around builds.
- Turning sccache off to reduce load (`RUSTC_WRAPPER=`). sccache's CPU time is attributed to the
  job that caused it.

## Taskfiles and other wrappers

`slot task check` holds one slot and passes the jobserver down, so cargo and make inside the task
share the pool. Wrapping the task is enough; the cargo calls inside it need no prefix.

A go command inside a wrapper is not capped, because go does not speak the jobserver protocol. If
a task runs large go builds, call `slot go …` inside the task instead.

## What not to wrap

- **rust-analyzer and IDE checks.** They run cargo for metadata and `cargo check` on their own
  schedule. Put the shim on `PATH` for agent shells only, never for the editor.
- **Long-running processes.** A server, a watcher or `cargo run` of a daemon holds its slot for as
  long as it runs. Start those without `slot`.

## Tests that need local services

Tests that need docker, a database or a port on localhost work through the local daemon, since
the job runs on the same machine. With a [remote server](remote-builds.md) they fail there. List
them in `.slot.toml` at the repository root, and they run on this machine's daemon while every
other build goes to the server:

```toml
local = ["cargo test -p integration", "go test ./e2e/..."]
```

Each entry is a command prefix: it matches when its words are the first words of the command, so
`cargo test -p integration -- --nocapture` matches and `cargo test -p integration2` does not. The
command still runs through the pool, and slot prints one line saying why it ran locally.
