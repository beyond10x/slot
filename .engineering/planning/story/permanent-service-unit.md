---
format: aep.planning-md/3
id: story:permanent-service-unit
kind: story
status: draft
title: The installed user unit replaces the transient one and survives reboot
relations:
- decomposes: epic:adoption-measured
- serves: vision:O6
scope:
- confidence: inferred
  path: website/docs/getting-started.md
revision: 2
---
# Story: permanent-service-unit

## Outcome

On the operator's workstation the daemon runs from the installed user unit
`~/.config/systemd/user/slot.service` (installed by `task service` from `contrib/slot.service`),
enabled, replacing the transient unit it runs under today, and it is running again after a reboot
without anybody starting it.

## Context

- `task service` exists (`Taskfile.yml`): installs the unit, `daemon-reload`, `enable --now`.
- `contrib/slot.service` is `WantedBy=default.target`: a user unit starts at login unless the user
  has lingering enabled (`loginctl enable-linger`), in which case it starts at boot.
- The transient unit has to be stopped first; two daemons would fight over the socket and port
  `127.0.0.1:7411`.

## Acceptance

1. After a reboot and before any manual start: `systemctl --user is-enabled slot.service` prints
   `enabled`, `systemctl --user is-active slot.service` prints `active`, and
   `systemctl --user show slot.service -p FragmentPath` names the file under
   `~/.config/systemd/user/`, not one under `/run/user/`.
2. `curl -fsS http://127.0.0.1:7411/api/state` exits 0.
3. `systemctl --user list-units --all --no-legend 'run-*'` lists no unit running `slot serve`.
4. The job log from before the switch is still read (`history_survives_restart` behaviour): the
   UI shows jobs from before the reboot.

## Scope

- `contrib/slot.service`, `Taskfile.yml` (`service`) only if the install exposes a defect
- `website/docs/getting-started.md` (the install step and lingering)
