---
format: aep.planning-md/3
id: story:the-proc-scan-allocates-nothing-per-process
kind: story
status: draft
title: The scan of /proc allocates nothing per process
relations:
- serves: vision:O6
revision: 1
---
## Outcome

The daemon's scan of `/proc` allocates no memory per process in the steady state, so a daemon
built for musl starts as many jobs per second as one built for glibc.

## Why

Measured on 2026-11-26 with the release binaries of both targets on one host: 5 rounds of 400
`true` jobs on 16 slots, median per daemon build.

- glibc daemon: 85.7 jobs/s, 4.87 s of daemon CPU per round.
- musl daemon: 75.4 jobs/s (12 % fewer), 5.82 s of daemon CPU per round (20 % more).
- A profile of the musl daemon in that burst: 45 % of its samples are in the allocator and its
  lock (`__libc_free` 12.1 %, `__lock` 8.4 %, `__libc_malloc_impl` 8.4 %, `alloc_slot` 5.6 %,
  `__libc_realloc` 5.3 %, `__unlock` 5.3 %). The glibc daemon: 10 % (`realloc` 5.2 %, `malloc`
  2.8 %, `cfree` 2.1 %).
- In both profiles the callers are `procstat::Snapshot::scan`: one `read_to_string` and one
  `Vec<&str>` of the split fields per process and scan.

The docs say to take the glibc archive where it runs, so no host is slower today. A second
allocator for the musl build alone was not taken: it adds a dependency and leaves the glibc
build's 10 % where it is.

## Acceptance

- Red first: a test that counts allocations of one `Snapshot::scan` over a fixed number of
  processes fails on the current code (a counting allocator in the test binary, no new
  dependency in the daemon).
- After the change a scan reuses one read buffer and parses the fields of `stat` without
  collecting them: the count per scan does not grow with the number of processes once the
  buffers have their size.
- The burst measurement above is repeated with both builds, 5 rounds each, and both medians are
  in the change's message; the musl daemon's median is within 5 % of the glibc daemon's.
- The scan's results are unchanged: the existing `procstat` tests pass without an edit to an
  expected value.
