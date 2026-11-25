---
format: aep.planning-md/3
id: story:the-node-has-musl-gcc-and-mingw-by-pinned-source
kind: story
status: draft
title: The node has musl-gcc and mingw by pinned source
summary: provision.sh installs musl-gcc and a mingw cross compiler from pinned archives checked by SHA-256; probe first.
relations:
- serves: vision:O6
- decomposes: epic:compute-cluster
revision: 1
---
## Outcome

`provision.sh apply` gives a build node the C compilers its consumers' gates call that Amazon Linux
2023 does not package: `musl-gcc` for x86_64, `x86_64-w64-mingw32-gcc` for the Windows GNU target,
and, if a consumer still needs it after surf answers, an aarch64 musl compiler. Each is installed
from a pinned release or source archive checked against its SHA-256, and `check` names a missing
one. Nothing is copied from the workstation.

## Evidence (2026-11-25)

- `contrib/node/README.md` lists mingw cross compilers, `musl-gcc`, `dash`, `mawk`, `erlang`,
  `elixir`, `odin` and `gh` as not installed.
- connectors' gate fails on the node without the mingw compiler; surf asks for musl-tools for the
  x86_64 and aarch64 musl targets; substrate needs no musl C compiler.
- Until this story is built, connectors' jobs stay in the host pool through
  `~/.config/slot/remote-deny-origins`.

## Recommendation, in order

1. Probe first, read-only, in a scratch directory on the node: extract the llvm-mingw release asset
   and run `ldd` on its binaries, `x86_64-w64-mingw32-gcc --version` (does the release ship
   gcc-named executables?) and one C compile for `x86_64-pc-windows-gnu` with the `cc` crate. The
   release is the Linux x86_64 ucrt asset of `mstorsjo/llvm-mingw` (tag 20261117 at the time of
   writing); its README says the binaries are built on Ubuntu and "hopefully" run elsewhere, which
   the probe settles on Amazon Linux 2023's glibc.
2. musl for x86_64: the musl source tarball from musl.libc.org, its signature checked once when
   the pin is chosen, its SHA-256 in `provision.sh`, built as root on the node with `./configure
   --prefix=/usr/local/musl-<version> && make && make install`, `musl-gcc` linked in
   `/usr/local/bin`. This needs a new "build from a source pin" step in `provision.sh` with a test
   (stubs, as for the archive pins). To confirm when it is built: that `make install` installs the
   `musl-gcc` wrapper.
3. mingw: the llvm-mingw release of step 1 as an `ARCHIVE_PINS` line, with
   `x86_64-w64-mingw32-gcc` linked to its clang wrapper if the probe shows no gcc-named executable.
4. aarch64 musl: decided after surf says whether a local task needs it or only the forge release
   does. The musl.cc archive (a rolling file of one maintainer, listed 2021, GCC 11.2.1) is the
   last choice and is not used without the host operator's word.

## Acceptance

- `push.sh check` and `provision.sh check` print 0 drift on a node with both compilers; `check`
  names each missing one.
- On the node, `musl-gcc` compiles a C file that includes `<stdio.h>` and the result runs; the mingw
  compiler builds `ring` for `x86_64-pc-windows-gnu` in one job.
- Each pin's SHA-256 test fails when the archive's hash differs, and no downloaded file runs before
  the check (the existing wrong-SHA test names both).
- connectors' gate passes on the node; its line in `remote-deny-origins` is removed afterwards.
- Not started before the node-provisioning pull request (`bare-node`) is merged.
