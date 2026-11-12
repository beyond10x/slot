---
title: Headroom
description: Keep a share of the CPU free for your own work, from a slider in the web UI or with slot headroom.
---

# Headroom

Headroom is the share of the CPU that builds leave free for you: the browser, a video call, the
editor. It goes from 0 to 50 % and applies at once, to every build on the machine that runs
through slot.

## Setting it

In the web UI, move **headroom for you** in the header. The caption shows what builds get, for
example `builds may use 14 of 20 cores, 14 of 20 slots` at 30 %.

From a shell:

```shell-session
$ slot headroom 30
headroom 30%: builds get 14 of 20 slots, cpu quota 1400% (14.0 cores)
$ slot headroom
headroom 30%: builds get 14 of 20 slots, cpu quota 1400% (14.0 cores)
$ slot headroom 0
headroom 0%: builds get 20 of 20 slots, no cpu quota
```

The value is saved in `limits.json` in the state directory and survives a restart of the daemon.

## Call mode

While a call uses your microphone or your camera, the daemon raises the headroom to the call
value, 40 % by default, and puts the previous value back 2 minutes after the call ends. A
headroom already above the call value stays as it is.

```shell-session
$ slot headroom
headroom 40%: builds get 12 of 20 slots, cpu quota 1200% (12.0 cores)
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

Level meters and mixers capture too, so `pavucontrol`, `pwvucontrol`, `easyeffects`, `helvum`,
`qpwgraph`, `coppwr` and `noisetorch` are ignored. `slot serve --call-ignore a,b` replaces the
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

## Not covered

- Builds that do not run through slot. The web UI shows them as unmanaged cores in the host
  census.
- A daemon not started by systemd: only the slot limit applies, and `slot headroom` says
  `cpu quota not applied: not running in a systemd user unit`.
- Memory and disk IO. The headroom limits CPU only.

`slot serve --cpu-quota false` keeps the slot limit and never sets a quota.

## Who may change it

`PUT /api/limits` accepts `{"headroom_percent": N, "call_headroom_percent": M}` (either field
may be left out) only with `Content-Type: application/json`,
and refuses a browser request whose `Origin` is not the daemon's own page. A web page on another
site cannot change the headroom by posting to `127.0.0.1`. Any local program that can reach the
port can, as it can read the job list.
