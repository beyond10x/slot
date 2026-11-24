---
format: aep.planning-md/3
id: story:refused-compiler-calls-are-named-with-their-remedy
kind: story
status: active
title: The compiler calls the cache refuses are named per repository with their remedy
relations:
- serves: vision:O6
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-24T13:09:06Z", actor: "agent:slot", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-24T13:09:06Z", actor: "agent:slot", revision: 3}
---
# The compiler calls the cache refuses are named per repository with their remedy

## Why

On the build node, 12:19-13:19 CET on 2026-11-24, sccache refused 43.6 % of all calls as not
cacheable: crate-type 4,252, multiple input files 3,664, missing input 1,825, incremental 1,721;
of the Rust calls it could cache, 51.3 % were hits.

## Acceptance

- `slot cache` (or the usage page) lists, per repository, its refused calls by reason over a
  window, read from the cache counters slot records per minute and the jobs' `cache_env`.
- For each reason it names whether a repository can remove it and how: `incremental` (set
  `CARGO_INCREMENTAL=0` in jobs that do not edit and rebuild, as a CI runner does), `crate-type`
  (bin, proc-macro and cdylib crates that sccache does not cache), `multiple input files`, and
  `missing input`.
- Where slot itself can remove a reason for every job (incremental off on a node), it does so
  and the measurement after shows the change.
- Red first: a counter record with each reason yields the per-repository table.
