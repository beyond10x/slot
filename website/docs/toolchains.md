---
title: Toolchains
description: How slot classifies a command as rust, go or other, what each toolchain sets, and how to add one.
---

# Toolchains

A toolchain decides, for one kind of command, how many slots a job holds, which variables the job
gets, which caller variables cross to a remote server, and which directories are not synced. The
daemon picks the first toolchain whose command names match the base name of the first word of
the command. `other` matches anything and comes last.

The toolchain's name is the `toolchain` field of a job in the job log and the colour key in the
web UI.

## Kinds of tools

How a tool shares the pool depends on whether it speaks the jobserver protocol:

| kind | examples | slots held | how parallelism is capped |
|---|---|---|---|
| jobserver client | cargo, rustc, GNU make 4.4+ | 1 | the tool reads more tokens from the shared FIFO |
| own parallelism flag | go (`-p`); gradle, bazel, pytest-xdist would be the same | a fixed number | the tool's own flag is set to the slots held |
| wrapper | task, make targets, just | 1 | the jobserver is passed down; clients inside share the pool, other tools inside are not capped |

## rust

- **Commands:** `cargo`, any `cargo-*` binary, `rustc`.
- **Slots:** 1. cargo and rustc read further tokens from the FIFO.
- **Sets:** `MAKEFLAGS` and `CARGO_MAKEFLAGS` with the jobserver; `RUST_TEST_THREADS` and
  `NEXTEST_TEST_THREADS` to `slot serve --test-threads` (default a quarter of the pool, at least
  2), unless already set. A test binary does not read the jobserver, so without this it starts
  one thread per core while its job holds one slot. In remote mode,
  `CARGO_TARGET_DIR=<cache-root>/target/<workspace-id>`.
- **Crosses to a remote server:** `CARGO_*` except `CARGO_TARGET_DIR`, `CARGO_HOME` and
  `CARGO_BUILD_JOBS`; `RUST*`; `NEXTEST_*`; `INSTA_*`; `SQLX_OFFLINE`.
- **Not synced:** `/target/`.

## go

- **Commands:** `go`, `gotestsum`, `golangci-lint`.
- **Slots:** `slot serve --go-tokens`, default 4, at most the pool size.
- **Sets:** `GOFLAGS` gets `-p=<slots held>` appended to any value the caller had;
  `GOMAXPROCS=<slots held>` unless already set, which also caps `t.Parallel()` tests inside one
  package; the jobserver variables are removed. In remote mode, `GOCACHE=<cache-root>/go-build`,
  shared by all workspaces (the Go build cache is safe for concurrent use).
- **Crosses to a remote server:** `GO*` except `GOCACHE`, `GOPATH`, `GOROOT`, `GOMODCACHE` and
  `GOTMPDIR`; `CGO_ENABLED`.
- **Not synced:** nothing beyond `.gitignore` and `.git`.

```shell-session
$ slot go env GOFLAGS
-p=4
```

## node-test

- **Commands:** `playwright test`, `vitest` and `jest`, also behind `npx`, `yarn`, `pnpm exec` or
  `bunx`, or as a path such as `node_modules/.bin/vitest`.
- **Slots:** one per worker: the value of `--workers` or `-j` (playwright), or `--maxWorkers` or
  `--max-workers` (vitest, jest), as a number or a percentage of the cores. Without one of those
  flags, 4. Pass the flag: without it the runner still starts about one worker per core.
- **Sets:** nothing; the jobserver variables are removed.
- **Crosses to a remote server:** `CI`, `NODE_ENV`, `NODE_OPTIONS`, `PLAYWRIGHT_*`, `VITEST_*`,
  `JEST_*`.
- **Not synced:** `/node_modules/`.

A browser started by a playwright worker uses more than one core; one slot per worker undercounts
it.

## other

- **Commands:** everything else, for example `task`, `make`, `just`, `sh`.
- **Slots:** 1.
- **Sets:** `MAKEFLAGS` and `CARGO_MAKEFLAGS` with the jobserver, so cargo or make started inside
  shares the pool; `RUST_TEST_THREADS`, `NEXTEST_TEST_THREADS` and `GOMAXPROCS` to
  `slot serve --test-threads`, unless already set, so test binaries and go commands inside a
  wrapper are capped too. In remote mode, `CARGO_TARGET_DIR` and `GOCACHE` as for rust and go.
- **Crosses to a remote server:** the union of the rust and go lists.
- **Not synced:** `/target/`.

go reads `GOMAXPROCS` for its default `-p`, so `slot task check` that runs `go test` uses
`--test-threads` cores rather than all of them, while the wrapper holds one slot. Call
`slot go test` directly where go should hold its own slots.

A test runner behind a wrapper is not seen: `slot task ux:gate` that starts `playwright test`
holds one slot while its workers use as many cores as the runner starts (one node process at
1153 % CPU on 2026-11-12). Pass a worker flag inside the task, or call the runner directly
(`slot npx playwright test --workers=2`) so the job holds one slot per worker.

All toolchains also pass `TERM`, `NO_COLOR`, `CLICOLOR`, `CLICOLOR_FORCE`, `TZ`, `LANG` and
`LC_ALL` to a remote server. A name that looks like a credential (`CARGO_REGISTRY_TOKEN`) is
held back even when it matches a toolchain's prefix. `SLOT_ENV` adds more names; see
[Remote builds](remote-builds.md#environment).

## Adding a toolchain

A toolchain is one Rust type implementing the `Toolchain` trait in `crates/slot/src/toolchain.rs`,
plus one entry in `registry()` before `Generic`. The client, the daemon and the web UI read the
registry or the job's `toolchain` field, so no other part changes.

```rust
pub trait Toolchain: Send + Sync {
    fn name(&self) -> &'static str;
    fn matches(&self, argv: &[String]) -> bool;
    fn tokens(&self) -> usize;
    fn env(&self, c: &Context, env: Vec<String>) -> Vec<String>;
    fn client_env(&self, name: &str) -> bool;
    fn sync_excludes(&self) -> &'static [&'static str];
}
```

In short:

1. Pick the kind from the table above. A tool that reads `MAKEFLAGS` is a jobserver client.
2. `env`: cap the tool's parallelism to the slots held; put shared caches under the cache root
   when one is set.
3. `client_env`: the tool's own variable prefix and nothing else. Credentials never cross.
4. `sync_excludes`: the tool's output directories.
5. If the tool does work in a daemon outside the job's process tree, add the process name to
   `COMPILE_SERVERS` so its CPU time is attributed to the job.

The full checklist, including the tests to add, is in the design document,
[§ 8.4 Adding the next toolchain](https://github.com/beyond10x/slot/blob/main/docs/design.md#84-adding-the-next-toolchain--checklist).
