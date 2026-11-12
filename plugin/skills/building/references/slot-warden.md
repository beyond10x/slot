# slot-warden

Find the agent sessions on this machine that build outside the shared slot pool, report them, and
on request ask each to build through slot. Started by the operator (`/slot:warden [report|nudge|watch]`);
the mode defaults to `report`.

## Boundaries

- Read-only on the machine: never kill, renice or signal a process, never edit another session's
  files, settings or hooks, never start, stop or reload the slot daemon.
- A nudge is a message to the session, nothing more. Never nudge the session you run in.
- At most one nudge per session per 30 minutes, kept in `$XDG_STATE_HOME/slot/warden.json`
  (`~/.local/state/slot/warden.json` when unset): `{"<session id>": <unix seconds of last nudge>}`.
  Create the file when missing; only this role writes it.

## 1. Read

Run `slot sessions --json`. Each entry has `session`, `title`, `unmanaged_cores` (average over
`window_s` seconds, from the daemon), `cores` and `commands` (`pid`, `cmdline`, `procs`) running
outside the pool now. `daemon: false` means the daemon did not answer: cores are 0, commands are
still right; say so in the report.

Done when the JSON parsed. A `slot: unrecognized subcommand` error means slot is older than 0.4.0:
report that `slot:upgrade` is needed, and stop.

## 2. Judge each entry

- Empty `session`: a build outside any agent session (an editor, a terminal, rust-analyzer).
  Report it under "not an agent session"; never nudge.
- A command that is not a build the pool should run (a dev server, a watcher, `cargo run` of a
  daemon, rust-analyzer): report it as "long-running, left alone".
- Everything else is a finding: session id, title, cores outside slot, and per command the pid and
  the `slot …` command that would replace it (the command line with `slot ` in front; for vitest
  or jest add `--maxWorkers=4`, for `playwright test` `--workers=2`, when no worker flag is there).

Done when every entry is in one of the three groups.

## 3. Act by mode

- `report`: no messages. Go to step 4.
- `nudge`: for each finding whose session was not nudged in the last 30 minutes, send one message
  through the host's cross-session messaging (in Claude Code, `ListAgents` names the sessions and
  `SendMessage` sends): *"slot: your `<command>` (pid <pid>) builds outside the shared build pool,
  <cores> cores. Rerun it as `<slot command>`; the PreToolUse hook of the slot plugin refuses these
  commands, see slot:building."* Record the time in `warden.json`. A session the host cannot
  message is reported as "not reachable".
- `watch`: run `report` or `nudge` (the operator says which; `nudge` when unsaid) every 5 minutes
  for the time the operator names, one hour when unsaid, with the host's scheduling or a loop of
  `slot sessions --json` calls; report once at the end, not per round.

Done when every finding of this round was messaged, skipped by the 30-minute rule with its last
nudge time, or reported as not reachable.

## 4. Report

Verdict line first: `N sessions build outside slot` or `no builds outside slot`. Then one line
per finding: title or id, cores, pid and the `slot` command; then "messaged", "skipped, nudged at
<time>" or "not reachable". Then the not-an-agent-session and long-running lines, one each. No
other prose.
