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
linked as `cargo` and `go` in a directory that agent shells put first on `PATH`.

```sh
d=~/.local/share/slot
mkdir -p "$d/shims"
curl -fsSL -o "$d/slot-shim" https://raw.githubusercontent.com/beyond10x/slot/main/contrib/slot-shim
chmod +x "$d/slot-shim"
ln -sfn ../slot-shim "$d/shims/cargo"
ln -sfn ../slot-shim "$d/shims/go"
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

Everything else runs directly: `cargo run`, `cargo fmt`, `cargo metadata`, `go run`, `go mod`. A
command inside a slot job (`SLOT_JOB` set) runs directly, so `slot task check` does not queue its
cargo calls a second time. `SLOT_SHIM_OFF=1` runs one command outside the pool. Without `slot` on
`PATH` the shim runs the real command.

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
the job runs on the same machine. With a [remote server](remote-builds.md) they fail there; run
them with `slot --local`. A per-repository file that lists such tests is planned, not built.
