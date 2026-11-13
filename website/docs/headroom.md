---
title: Headroom
description: Keep a share of the CPU free for your own work, from a slider in the web UI or with slot headroom.
---

# Headroom

Headroom is the share of the CPU that builds leave free for you: the browser, a video call, the
editor. It goes from 0 to 50 % and applies at once, to every build on the machine that runs
through slot.

## Setting it

In the web UI, press **0 %**, **20 %** or **40 %** next to **headroom for you** in the header, or
move the slider for another value. The caption shows what builds get, for example `builds may use
14 of 20 cores, 14 of 20 slots` at 30 %. If the daemon refuses the change (a daemon older than
0.3.0 has no headroom), the header says why and the slider goes back to the value the daemon runs.

From a shell:

```shell-session
$ slot headroom 30
headroom 30% of the pool: builds get 14 of 20 slots, cpu quota 1400% (14.0 cores)
$ slot headroom
headroom 30% of the pool: builds get 14 of 20 slots, cpu quota 1400% (14.0 cores)
$ slot headroom 0
headroom 0% of the pool: builds get 20 of 20 slots, no cpu quota
```

The value is saved in `limits.json` in the state directory and survives a restart of the daemon.

## Call mode

While a call uses your microphone or your camera, the daemon raises the headroom to the call
value, 40 % by default, and puts the previous value back 2 minutes after the call ends. A
headroom already above the call value stays as it is.

```shell-session
$ slot headroom
headroom 40% of the pool: builds get 12 of 20 slots, cpu quota 1200% (12.0 cores)
call mode 40%: call on (Chromium), headroom 40%, back to 20% after the call
$ slot headroom --call 30
$ slot headroom --call 0
```

`--call 0` turns call mode off; the value is saved like the headroom. The web UI shows a
**call** badge with the applications in the call, and `slot status` a `call mode:` line.

Moving the slider or running `slot headroom N` during a call sets the headroom by hand: that
value stays, and the end of the call does not change it.

A call is any of:

- a PipeWire capture stream in state `running` with an active link from a microphone or camera
  node (`pw-dump`, every 5 seconds). A browser tab that keeps the microphone open but muted or
  corked is `idle`, so it does not count; neither does a recording of a sink monitor.
- a process of yours that holds `/dev/video*` open. Browsers read the camera this way.

Level meters, mixers and dictation capture too, so `claude` (Claude Code's voice input),
`pavucontrol`, `pwvucontrol`, `easyeffects`, `helvum`, `qpwgraph`, `coppwr` and `noisetorch` are
ignored. A name in the list matches the application name, its binary, the name of its process,
or a directory or file name in the path of its executable: Claude Code runs as
`.../claude/versions/<version>`, so its process name is a version number. An ALSA program shows
up in PipeWire as `PipeWire ALSA [...]`, so its process name is the name shown. `slot serve --call-ignore a,b` replaces the
list; `--call-headroom` sets the default call value and `--call-off-after` the delay in seconds.
Without `pw-dump` only camera use is seen, and `slot headroom` says so.

## What it changes

Two limits, both lifted again at 0 %:

- **Slots.** Of the pool's tokens, `tokens × (100 − headroom) / 100` circulate (at least 1). The
  daemon takes the rest out of the jobserver as running builds return them, so the effect grows
  over seconds: a token a compiler holds cannot be taken back. Raising the slider again writes the
  withheld tokens back at once. A queued job that asked for more slots than now circulate is
  given the new maximum.
- **CPU quota.** When the daemon runs as a systemd user service, it sets `CPUQuota` on its own
  unit to `(100 − headroom) × cores` percent, where cores are the CPUs online on the host
  (`/sys/devices/system/cpu/online`), never the quota in force. Every job runs inside that unit's
  cgroup, so the quota caps all builds together, including test binaries and anything else a job
  starts that the jobserver does not see. The kernel enforces it immediately. With
  `--memory-max`, jobs run in scopes under `slot.slice` instead, and the same quota is set there.

The slot count shapes the queue; the quota is what holds the CPU free.

## Headroom of the machine

The quota above caps the pool only. A docker container a build started, a compile server
outside the daemon's unit and your desktop all run outside it, so at 45 % the machine can still
be 95 % busy. With the machine base the headroom is kept free on the whole machine:

```shell-session
$ slot headroom --base machine
headroom 45% of the machine: builds get 11 of 20 slots, cpu quota 1100% (11.0 cores) now, following the CPU used outside the pool
$ slot status
11 of 20 slots (45% headroom), 2 free, 4 running, 0 queued
machine (over target): 18.2 of 20 CPUs busy, target 11.0, pool quota 2.0 (floor), outside 9.1: session-3.scope 5.9, docker 3f2a1b2c3d4e 3.2
$ slot headroom --base pool
```

Each second the daemon reads the CPUs busy on the host (`/proc/stat`) and the CPU of its own
cgroup (`cpu.stat`). The difference is the CPU used outside the pool, averaged over about 30
seconds. The pool's quota is then the target, `cores × (100 − headroom) / 100`, less that
outside load, never below 2 CPUs and never above the target. The daemon sets a new quota only
when it moved by half a CPU or more, at most every 10 seconds. The slot count is the same as on
the pool base.

`slot status` and the web UI header show the line above: what is busy, the target, the quota
now, and the five busiest owners outside the pool (applications in your `app.slice`, login
sessions, docker containers, `system.slice`). `over target` means the pool is down to its
floor and the machine is still busier than the target: what runs outside the pool uses the
headroom, and slot cannot take it back.

The base is saved with the headroom. `slot headroom --base pool` puts the fixed quota back.

## Not covered

- Builds that do not run through slot. The web UI shows them as unmanaged cores in the host
  census. On the machine base their CPU lowers the pool's quota.
- A daemon not started by systemd: only the slot limit applies, and `slot headroom` says
  `cpu quota not applied: not running in a systemd user unit`.
- Memory and disk IO. The headroom limits CPU only.

`slot serve --cpu-quota false` keeps the slot limit and never sets a quota.

## Who may change it

`PUT /api/limits` accepts `{"headroom_percent": N, "call_headroom_percent": M,
"headroom_base": "machine"}` (any field may be left out) only with `Content-Type: application/json`,
and refuses a browser request whose `Origin` is not the daemon's own page. A web page on another
site cannot change the headroom by posting to `127.0.0.1`. Any local program that can reach the
port can, as it can read the job list.
