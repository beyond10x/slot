---
format: aep.planning-md/3
id: story:releases-carry-static-musl-binaries
kind: story
status: draft
title: Releases carry static musl binaries
revision: 1
---
## Outcome

Every slot release carries static Linux binaries (`x86_64-unknown-linux-musl` and
`aarch64-unknown-linux-musl`) that run on a host whatever C library it has.

## Why

The release workflow builds the two gnu targets on the runner's image; such a binary names the
C library versions of that image. Release binaries of another tool of the organization, built
the same way, print `weak version 'GLIBC_2.39' not found` on every call on a host with glibc
2.34. The operator approved static musl releases for all Rust tools of the organization on
2026-11-25.

## Acceptance

- The release workflow builds, smoke-tests and packages both musl targets beside the gnu ones;
  `SHA256SUMS` lists them.
- CI proves each musl binary is static (`file` or `readelf -d` shows no dynamic section's
  `NEEDED` entry) and that `slot --version` answers the release's version in an Alpine
  container.
- The daemon's integration tests that read `/proc` and hand over descriptors pass on the
  x86_64 musl build.
- `contrib/node/provision.sh` can install either archive; which one a node takes is a config
  value, the default unchanged until a node ran the musl build under real jobs.
- Red first: a check of the release assets that expects the musl archives fails before the
  change.

## Notes

- Time `cargo build --release` for a musl target before and after: the allocator of musl is
  slower under threads; measure the daemon under a job burst and name the result.
