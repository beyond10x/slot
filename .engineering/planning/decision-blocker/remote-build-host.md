---
format: aep.planning-md/3
id: decision-blocker:remote-build-host
kind: decision-blocker
status: open
title: No machine has been chosen as the remote build server
relations:
- blocks: story:remote-mode-on-a-real-server
revision: 2
---
# Decision blocker: remote-build-host

`story:remote-mode-on-a-real-server` needs a second Linux machine reachable over ssh, and nobody
has decided which one: `docs/design.md` § 5 item 4 says "Phase 0 measures core-hours before any
hardware is chosen", and no host is named anywhere in the repository.

## The question

Which machine is the build server for the verification (an existing machine, or one to be
provisioned once the job log has sized it), and who gives the workstation's key ssh access to it?

## Clears when

The operator names the machine and `ssh <it> true` exits 0 from the workstation. Installing slot
on it is part of `story:remote-mode-on-a-real-server`.
