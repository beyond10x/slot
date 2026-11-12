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
  starts that the jobserver does not see. The kernel enforces it immediately.

The slot count shapes the queue; the quota is what holds the CPU free.

## Not covered

- Builds that do not run through slot. The web UI shows them as unmanaged cores in the host
  census.
- Jobs started with `slot serve --memory-max`: each runs in its own `systemd-run --scope` unit,
  outside the daemon's unit, so the quota does not reach them. The slot limit still does.
- A daemon not started by systemd: only the slot limit applies, and `slot headroom` says
  `cpu quota not applied: not running in a systemd user unit`.
- Memory and disk IO. The headroom limits CPU only.

`slot serve --cpu-quota false` keeps the slot limit and never sets a quota.

## Who may change it

`PUT /api/limits` accepts `{"headroom_percent": N}` only with `Content-Type: application/json`,
and refuses a browser request whose `Origin` is not the daemon's own page. A web page on another
site cannot change the headroom by posting to `127.0.0.1`. Any local program that can reach the
port can, as it can read the job list.
