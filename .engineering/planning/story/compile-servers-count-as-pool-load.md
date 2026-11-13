---
format: aep.planning-md/3
id: story:compile-servers-count-as-pool-load
kind: story
status: draft
title: On the machine base, compile servers of pooled jobs count as pool load
relations:
- decomposes: epic:desktop-headroom
revision: 1
---
## Outcome

On the `machine` headroom base, CPU used by compile servers that serve the pool's own jobs counts
as pool load, not as load outside the pool. The quota then falls only for work the pool did not
start, so an sccache server busy with the pool's compiles does not halve the pool.

## Context

- `server/machine.rs` `pool_cgroups` returns the daemon's own cgroup and `slot.slice`, nothing
  else; no setting adds a unit. Outside load = machine busy − those cgroups.
- A pooled `cargo` with `RUSTC_WRAPPER=sccache` hands each rustc to an sccache server. When that
  server was started outside the daemon (a user unit such as `sccache.service`, or a session's
  shell), its compiles run in that server's cgroup. Measured on a 20-CPU host: one such unit used
  up to 4.2 CPUs at once, all of it compile work of pooled jobs; the machine base would have cut
  the quota from 11 to about 5.4 CPUs.
- A server inside a session scope shares the scope with that session's other processes, so the
  cgroup alone cannot separate it; the server's process tree can (its pid and its rustc children).

## Acceptance

1. `slot serve --pool-unit <unit>` (repeatable) and the same list in the saved limits add a
   cgroup under the user manager to the pool's side of the computation; `slot status` lists them.
2. sccache servers the daemon already watches (`sccache.rs`) that sit outside the pool's cgroups
   count with their child processes' CPU as pool load, read from `/proc/<pid>/stat`, and are not
   counted twice when their unit is also listed.
3. A test on a temp cgroup tree and a fake `/proc` shows: busy 15, pool 6, a listed unit 4 →
   outside 5, quota = target − 5.
4. The status line names compile-server CPU apart from the rest of the pool.

## Scope

- `crates/slot/src/server/machine.rs`, `crates/slot/src/server/mod.rs`, `crates/slot/src/sccache.rs`
- `crates/slot/src/cli.rs`, `crates/slot/src/status.rs`, `website/docs/headroom.md`
