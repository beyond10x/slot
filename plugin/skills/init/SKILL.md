---
name: init
description: Start with slot on this machine — install the `slot` CLI, run its daemon as a systemd user service, and take the first build through it. slot is one shared pool of build slots for every build on a machine (one GNU-make jobserver, a queue and a live web view), optionally on a build server. Use when the user wants to stop parallel agent builds from oversubscribing the machine, asks to set up or install slot, or when `slot:building` reports that `slot` or its daemon is missing. Installs the CLI and the daemon only after the user confirms.
---

# Start with slot

slot runs on Linux only (x86-64 and ARM64); it reads `/proc`. On another system say so and stop.

## 1. Have the CLI

Run `slot --version`. It prints `slot <version>`: go to step 2, and use `slot:upgrade` later for
newer releases. Missing: tell the user you will install the newest release archive into
`~/.local/bin` and ask once. After a yes, with the newest version from
`curl -fsSLI -o /dev/null -w '%{url_effective}\n' https://github.com/beyond10x/slot/releases/latest`
(the last path segment) and the target from `uname -m` (`x86_64` or `aarch64`):

```bash
version=<newest>
target=$(uname -m)-unknown-linux-gnu
dir=$(mktemp -d)
cd "$dir"
base=https://github.com/beyond10x/slot/releases/download/$version
curl -fsSLO "$base/slot-$version-$target.tar.gz"
curl -fsSLO "$base/SHA256SUMS"
sha256sum --check --ignore-missing SHA256SUMS
tar -xzf "slot-$version-$target.tar.gz"
install -D -m 0755 "slot-$version-$target/slot" ~/.local/bin/slot
```

`sha256sum` prints `OK` for the archive; anything else, stop and report it. When the user prefers
a source build and a Rust toolchain is on `PATH`:
`cargo install --locked --root ~/.local --git https://github.com/beyond10x/slot b10x-slot-cli`
(its web view is a placeholder page). Done when `slot --version` prints the version.

## 2. Have the daemon

One daemon serves every session of the user. `curl -fsS http://127.0.0.1:7411/api/state` printing
JSON with `tokens` and `free` means it runs; go to step 3. Otherwise ask the user once, then
install the unit from the repository, so the daemon outlives this session and no session owns it:

```bash
mkdir -p ~/.config/systemd/user
curl -fsSL -o ~/.config/systemd/user/slot.service \
  https://raw.githubusercontent.com/beyond10x/slot/main/contrib/slot.service
systemctl --user daemon-reload
systemctl --user enable --now slot.service
```

The unit starts `~/.local/bin/slot serve`; installed elsewhere, change `ExecStart`. The defaults
are one slot per CPU, go jobs holding 4 and the web view on `127.0.0.1:7411`; other figures go on
`ExecStart` (`slot serve --help` lists them). Without systemd the user starts `slot serve` in a
terminal of their own: it does not exit, so it is never a foreground command of this session.

Done when the `curl` line answers. Give the user the live view: <http://127.0.0.1:7411/>.

## 3. First build here

Run one build of this repository through the pool (`slot cargo build`, `slot task check`) and
confirm it prints the usual output and exits with the usual code. From now on follow
`slot:building`.

## 4. Make it the default (on request only)

- One repository: add to its `AGENTS.md` *Build through slot: prefix builds with `slot`
  (`slot cargo test …`); `slot --local` bypasses the daemon.*
- Every agent session: the `cargo`, `go` and `ignite` PATH shim from
  <https://beyond10x.github.io/slot/docs/agent-integration> sends their build subcommands through
  slot without a prefix, for agent shells only.
- Claude Code: this plugin's `PreToolUse` hook refuses a Bash command that builds or tests outside
  the pool and names the `slot` command to run instead (`slot hook claude-pre-tool-use`, slot 0.4.0
  or newer). `SLOT_HOOK=warn` in the session's environment turns the refusal into advice;
  `SLOT_HOOK=off` turns it off.
- Sessions that still build outside the pool: `/slot:warden` (`slot:building` § The warden).

## Next

- Build through the shared pool: `slot:building`.
- Later, `slot:upgrade` checks for a newer plugin and CLI.
