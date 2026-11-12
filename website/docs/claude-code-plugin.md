---
title: Claude Code plugin
description: Install the slot plugin for Claude Code - skills for building through slot, a PreToolUse hook that refuses builds outside the pool, and the slot-warden agent.
---

# Claude Code plugin

The slot repository is a Claude Code plugin marketplace. Its one plugin, `slot`, carries:

- skills: `slot:building` (how to build through slot), `slot:init` (install the CLI and the
  daemon), `slot:upgrade`, and `/slot:warden`;
- a `PreToolUse` hook that refuses a Bash command which compiles or runs tests outside the pool
  and names the `slot` command to run instead;
- the `slot-warden` agent, which finds sessions that still build outside the pool.

The skills also load in Codex (`.codex-plugin/plugin.json`); the hook and the agent are Claude Code
only.

## Install

The hook needs slot 0.4.0 or newer on `PATH`; with an older slot or none it allows everything.

```console
claude plugin marketplace add beyond10x/slot
claude plugin install slot@slot
```

The plugin loads in sessions started after the install.

## The hook

Claude Code runs `slot hook claude-pre-tool-use` before every Bash command. The command reads the
event on stdin, splits the command line into simple commands (on `&&`, `||`, `;`, `|`, `&`,
newlines and parentheses; inside `bash -c '…'` too) and checks each:

- `cargo` build, check, test, clippy, doc, bench, nextest, install, rustc, llvm-cov, miri;
- `go build`, `test`, `vet`, `install`;
- `make -j…`, and `make` or `task` targets whose name has a word such as `check`, `test`, `build`,
  `lint`, `gate`, `verify`, `generate` or `all` (split on `:`, `-`, `_`, `.`, `/`), unless it also
  has `deploy`, `dev`, `up`, `watch`, `serve`, `start`, `push` or `merge`;
- `npm test`, `yarn test`, `pnpm test` and `run test`;
- vitest, jest and `playwright test`, through `npx`, `bunx`, `yarn`, `pnpm`, `node` or
  `node_modules/.bin`.

A match is refused: the hook exits 2 and Claude Code shows the agent one line, for example

```text
slot: `npx vitest run` compiles or runs tests outside the shared build pool. Run `slot npx vitest run --maxWorkers=4` instead (the job holds one slot per worker, so name the worker count).
```

These pass:

- anything that starts with `slot`, or has `SLOT_SHIM_OFF=1` in front;
- every command inside a slot job (`SLOT_JOB` set), so `slot task check` is not refused for the
  cargo calls inside it;
- `cargo` and `go` when the [PATH shim](agent-integration.md#make-it-the-default-the-path-shim)
  is installed in `~/.local/share/slot/shims`, since the shim sends them through slot;
- quoted text, comments and heredoc bodies (`git commit -m "cargo test"`, `cat <<'EOF' … EOF`);
- dry runs and listings: `make -n`, `task --list`, `cargo --version`, `cargo fmt`, `cargo run`,
  `go mod tidy`, `npx playwright install`.

A command the hook cannot read is allowed. Command substitutions inside double quotes are not
examined.

`SLOT_HOOK` in the environment of Claude Code changes the mode:

- unset or `block`: refuse, as above;
- `warn`: allow the command and add the same line to the agent's context;
- `off`: allow everything.

## The warden

`slot sessions` lists the agent sessions that build outside the pool: cores outside slot averaged
over the last 60 seconds of the daemon's history (`--window`), and the commands running outside
it now, grouped under the command the session's shell started.

```text
5c1f… "ui pass": 3.2 cores outside slot, 3.4 total (avg 60 s)
  pid 48211 (9 procs): node node_modules/.bin/vitest run
```

`--json` prints the same as JSON. `--min-cores` (default 0.5) leaves out sessions below it that
run nothing outside the pool now, and build processes outside any agent session below it.

`/slot:warden` runs the `slot-warden` agent on that list in one of three modes:

- `report` (default): one line per session with the `slot` command that replaces each build;
- `nudge`: also messages each session once, at most once per 30 minutes per session;
- `watch`: repeats every 5 minutes for the time given, one hour by default.

The warden never kills a process, edits another session's files or touches the daemon.
