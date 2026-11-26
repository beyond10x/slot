---
format: aep.planning-md/3
id: story:a-crate-built-in-one-worktree-is-served-from-the-compile-cache-in-the-next
kind: story
status: draft
title: A crate built in one worktree is served from the compile cache in the next
revision: 2
---
## Outcome

A dependency crate that one worktree of a repository compiled on a build node is served from the
shared compile cache when another worktree of that repository builds it, whether cargo runs as a
job of its own or inside a generic job. A consumer repository's first build in a new worktree
compiles its own crates and nothing else that the node has built before.

## Why (measured numbers)

Measured 2026-11-26 on a build node with sccache 0.18.0, same-path mirrors, on one lane of a
consumer repository (wasm32-unknown-unknown, 93 rustc calls through the cache, eight crates of
one git dependency on the critical path):

- First build in a worktree: 1 min 46 s, the eight crates 86.67 s. The same build again on an
  empty target directory in the same worktree: 18.15 s, the eight crates 11.03 s. The cache works.
- The same build in a second, new worktree: 2 min 13 s, the eight crates 105.61 s. Nothing was
  served, although the hashed part of all 93 rustc command lines is byte-identical.
- sccache puts every `CARGO_*` variable (except `CARGO_MAKEFLAGS`, `CARGO_REGISTRIES_*`,
  `CARGO_BUILD_JOBS`, `CARGO_ENCODED_RUSTFLAGS`), every variable a crate reads with `env!`, the
  working directory and the contents of every extern into a Rust key
  (`src/compiler/rust.rs`, `generate_hash_key`).
- slot sets `CARGO_TARGET_DIR=<tree>/target` for a same-path cargo job. One tree, two values of
  the variable: a crate without any path of its own compiled twice (0.68 s, 0.70 s). One tree,
  two target directories named by `--target-dir`, variable unchanged: served (0.73 s, 0.08 s).
- A cargo job and cargo inside a generic job do not share entries (0.83 s, 0.93 s): the cargo job
  carries `CARGO_TARGET_DIR` and `CARGO_NET_OFFLINE=true`, the generic job neither.
- With the variable held equal and only the target path changed, 41 of 67 library units were
  served and 26 compiled again: three crates that include a file from their build script's output
  directory (`# env-dep:OUT_DIR=<target>/…/out` in dep-info, the absolute path is inside their
  `.rmeta`) and every crate that depends on them, the eight crates among them (87.22 s).
- With the target directory at one absolute path for two worktrees and no `CARGO_*` variable in
  the job, the second worktree got the eight crates from the cache: 5.77 s instead of 105.61 s.
- A lane that sets a `CARGO_TARGET_<triple>_RUNNER` variable shares nothing with a lane that
  does not (88.98 s against 5.47 s for the eight crates, same tree, same target directory).
- A second profile name in one tree is a second `OUT_DIR`: 2.46 s, then 2.76 s for the first
  crate that includes from it; a consumer measured 157 s and 102 s for two such builds.
- The cache was not full: 47.3 GB of a 214.7 GB limit; an entry written 33 minutes earlier was
  served.
- Base directories of sccache do not apply to Rust in 0.18.0: only the C and C++ path uses them.
- A same-path workspace's target directory is not seeded from a sibling, and a target directory
  seeded on the workstation is not synced, so a new worktree has neither cargo's reuse nor the
  cache's.

## Acceptance

1. The probe `contrib/cache-key-probe.sh` builds one package twice against a live cache, each
   time with a `--cfg` name of its own so that the first build is a compile, reads the package's
   time from cargo's timing report, and exits 0 when the second build takes at most a third of
   the first. Modes: two worktrees (`tree`), two values of `CARGO_TARGET_DIR` (`var`), two
   `--target-dir` values (`flag`), a cargo job then cargo inside a generic job (`jobkind`). It
   runs outside the offline gate, in a lane that needs a live cache; each of its jobs ends within
   the pool's run-time limit.
2. `tree` on a crate without a build script passes. Today: 1.22 s, 1.19 s, exit 1.
3. `jobkind` on the same crate passes. Today: 0.83 s, 0.93 s, exit 1.
4. `tree` on a crate that depends on a crate including from `OUT_DIR` passes, on the host target
   and on wasm32-unknown-unknown. Today: 6.27 s, 5.24 s, exit 1.
5. `flag` on a crate without a build script still passes (today 0.73 s, 0.08 s), and a job whose
   caller set `CARGO_TARGET_DIR` itself still builds into that directory.
6. The reference page of a job's environment says which variables slot sets in a job and that
   each `CARGO_*` variable is part of the cache key.
7. The job record still carries the job's target directory; eviction and disk admission read it
   from there.
8. Each worktree keeps its own target data. The one absolute path is what a job sees: the job's
   own `<tree>/target` bound at that path for the job alone. No directory is shared by two
   worktrees. The probe has a mode for it (`both`): two worktrees on different branches build
   and run their tests at the same time; each runs the test binary of its own branch (a test
   that prints a value only that branch has), and neither waits on the other's cargo lock (the
   second job's build log has no `Blocking waiting for file lock` line).
9. The change on a node is not made before the operator saw the numbers of acceptance 2 to 5
   and 8 from the probe.

## Notes

- Two steps. First: do not export `CARGO_TARGET_DIR` when it is the directory cargo resolves by
  itself, and treat `CARGO_NET_OFFLINE` so that a cargo job and a generic job agree (acceptance
  2, 3, 5). Second: one absolute target path for every worktree of a repository (acceptance 4);
  the fixed-path option of the sandbox is the existing design, and it runs without the compile
  cache today because the cache server is outside the sandbox. sccache's client-side mode
  (`client_side_mode`), where the client process hashes and compiles and the server is only the
  storage gateway, is the candidate to lift that; it is not tested. Binding only the target
  directory at a fixed path is enough for dependency crates.
- Ruled out by the operator: one target directory shared by the worktrees of a repository. Two
  branches in one target directory overwrite each other's test binaries and wait on cargo's
  lock. The fixed path is a view inside the job's mount namespace.
- Not covered: a second profile name (its directory is part of `OUT_DIR` under any scheme); a
  variable the caller sets itself, such as a runner; clippy and check builds, which use another
  `--emit` and were not measured; workspace member crates, whose working directory is the tree
  unless the sources are bound at a fixed path too.
- Alternative for the first build of a new worktree: seed a same-path tree's target directory
  with the dependency units of a built sibling only; not tested.
- For a consumer repository until then: keep lanes in one profile directory, put a test runner
  in `.cargo/config.toml` instead of a lane's environment, and keep one feature set of a heavy
  dependency across lanes.
