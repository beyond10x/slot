---
name: upgrade
description: Check whether the slot plugin and the `slot` CLI are current, and upgrade them with the user's confirmation. Use when the user asks whether slot is up to date or to upgrade or update it, or when a `slot` command behaves differently from what a slot skill describes.
---

# Upgrade slot

1. Installed CLI: `slot --version`. Newest release:
   `curl -fsSLI -o /dev/null -w '%{url_effective}\n' https://github.com/beyond10x/slot/releases/latest`
   (the last path segment).
2. Plugin: `claude plugin marketplace update slot`, then `claude plugin list` shows the installed
   `slot@slot` version; in Codex, the plugin list of the host.
3. Both current: say "slot is current" with the versions, and stop. Otherwise list the
   differences in one list and ask once.
4. After a yes: the CLI is installed with `slot:init` step 1 at the newest version, over the old
   binary. The plugin: `claude plugin update slot@slot`; a new plugin version loads in a new
   session.

Done when `slot --version` prints the newest version and the plugin list shows the new plugin.

## The running daemon

A replaced binary does not change the daemon that is already running. `systemctl --user reload
slot` sends SIGHUP. A 0.11.0 or later daemon re-executes the new binary at once, same pid, and
hands it the running and queued jobs; the journal says `slot: handing … job(s)` and `slot: took
over …`. An older daemon re-executes at the first moment no job is active (`slot: idle,
re-executing`). Never `restart` for an
upgrade: it drains every session's builds and sends new ones outside the pool while it waits. A
build server's daemon is upgraded the same way on that machine.

## Next

- Continue the work that prompted the check; `slot:init` lists the skills.
