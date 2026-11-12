---
title: Getting started
description: Install slot, run the daemon as a systemd user service, and run a first build through it.
---

# Getting started

slot runs on Linux (x86_64 and aarch64). You install one binary, run it as a daemon, and prefix
build commands with `slot`.

## Install

### From a release archive

Each [release](https://github.com/beyond10x/slot/releases) carries one archive per target,
`slot-<version>-<target>.tar.gz`, for `x86_64-unknown-linux-gnu` and
`aarch64-unknown-linux-gnu`, plus `SHA256SUMS`. An archive holds a directory
`slot-<version>-<target>/` with the `slot` binary, `LICENSE` and `README.md`. The binary
includes the web UI.

```console
version=0.1.0
target=x86_64-unknown-linux-gnu
base=https://github.com/beyond10x/slot/releases/download/$version
curl -LO "$base/slot-$version-$target.tar.gz"
curl -LO "$base/SHA256SUMS"
sha256sum --check --ignore-missing SHA256SUMS
tar -xzf "slot-$version-$target.tar.gz"
install -D -m 0755 "slot-$version-$target/slot" ~/.local/bin/slot
```

The systemd unit below starts `~/.local/bin/slot`. If you install the binary elsewhere, change
`ExecStart` in the unit.

### With cargo

Rust 1.89 or newer:

```console
cargo install --locked --root ~/.local --git https://github.com/beyond10x/slot b10x-slot-cli
```

This installs `slot` into `~/.local/bin`, where the systemd unit looks for it. The web UI is built separately and is not in the
repository, so a binary installed this way serves a placeholder page instead of the UI. To get
the UI from source, clone the repository and run `task install`, which builds the UI with pnpm
and then installs the binary.

## Run the daemon

`slot serve` runs the daemon in the foreground. To run it as a systemd user service, install the
unit from [`contrib/slot.service`](https://github.com/beyond10x/slot/blob/main/contrib/slot.service):

```console
mkdir -p ~/.config/systemd/user
curl -L -o ~/.config/systemd/user/slot.service \
  https://raw.githubusercontent.com/beyond10x/slot/main/contrib/slot.service
systemctl --user daemon-reload
systemctl --user enable --now slot.service
```

From a checkout of the repository, `task service` does the same.

The unit sets `PATH` to `~/.cargo/bin`, `~/.local/bin`, `~/go/bin` and the system directories.
Local builds run with the caller's environment; the unit's `PATH` matters when the daemon serves
[remote clients](remote-builds.md).

By default the daemon:

- creates one slot per CPU (`--tokens`),
- listens on `$XDG_RUNTIME_DIR/slot.sock` (`--socket`, or `SLOT_SOCKET`),
- keeps its jobserver FIFO and job log in `$XDG_STATE_HOME/slot`, or `~/.local/state/slot`
  (`--state-dir`),
- serves the web UI on `127.0.0.1:7411` (`--http`, `off` disables it).

Open <http://127.0.0.1:7411/> to see the live view. All options are in the
[CLI reference](reference/cli.md#slot-serve).

## Run a build

Prefix the command with `slot`:

```console
slot cargo test -p foo
slot go test ./...
slot task check
```

The command runs in the current directory with the current environment, except for the variables
that would bypass the pool (`MAKEFLAGS`, `MFLAGS`, `CARGO_MAKEFLAGS`, `CARGO_BUILD_JOBS`), which
the daemon drops. stdout and stderr stream back as the command writes them, and `slot` exits with
the command's exit code.

When a job waited two seconds or more for a slot, `slot` prints one line on stderr after it ends:

```text
slot: job 3 waited 5s for a build slot, ran 0s
```

Ctrl-C, or any signal that ends `slot`, cancels the job: the daemon kills the job's process group
and returns its slots to the pool.

If the command's name is also a slot subcommand (`serve`, `proxy`, `run`), use `slot run`:

```console
slot run serve --port 8080
```

## When the daemon is not running

If `slot` cannot reach the daemon, it runs the command locally and prints one line on stderr:

```text
slot: daemon unreachable (connect <socket>: Connection refused (os error 111)), ran locally
```

The build still runs and keeps its exit code; it just does not share the pool. To skip the daemon
on purpose, use `--local`:

```console
slot --local cargo build
```

## Next

- [How it works](how-it-works.md): the pool, the queue, the job log and the web UI.
- [Agent integration](agent-integration.md): make coding agents use slot.
- [Remote builds](remote-builds.md): run builds on another machine.
