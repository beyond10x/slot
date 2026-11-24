---
format: aep.planning-md/3
id: story:the-jobserver-fifo-is-not-in-a-searched-directory
kind: story
status: draft
title: The jobserver FIFO is out of reach of a directory search, and a stray reader is named
relations:
- serves: vision:O2
- informed_by: story:lost-tokens-return-on-a-busy-pool
revision: 1
---
# Story: the-jobserver-fifo-is-not-in-a-searched-directory

## Outcome

A search over the daemon's state directory (`grep -r`, `grep -l dir/*`, `find -exec cat`) never opens the pool's jobserver FIFO. The FIFO lives in its own directory under the runtime directory, mode 0700, and nothing else is in that directory. When a process that is not a pool job and not the daemon holds the FIFO open for more than 10 s, `slot status` names it (pid, command, owner session) next to the lost-slot count.

## Evidence (2026-11-24, 08:34–08:36 CET)

- `grep -l` over every file in the state directory read the FIFO for about 126 s: `slot status` showed 29 lost slots of 32. The tokens returned after the grep was killed, through the lost-token recovery (story:lost-tokens-return-on-a-busy-pool).
- Nothing named the reader; its owner found it from its own sub-agent's log.

## Acceptance

- `grep -r . <state dir>` and `grep -l x <state dir>/*` finish within 1 s and take no token.
- A test opens the FIFO for reading from a process outside the pool: within 15 s `slot status` prints its pid and command.
