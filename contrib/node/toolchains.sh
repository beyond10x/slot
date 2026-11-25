#!/bin/sh
# toolchains.sh <usr-prefix> <rustup-home>: one line per toolchain a build job can resolve to.
#   rust <name> <rustc commit hash> <installed targets, clippy, rustfmt, rust-src and llvm-tools,
#        sorted and comma separated; rust-docs and the manifests are left out>
#   go <version> / node <version>
#   c <tool> <first line of its --version>, or `c <tool> missing`
# Run with prefix "" on the workstation and on the node: each prints what its own machine has.
# push.sh check compares the rust, go and node lines; the C tools are for reading.
export LC_ALL=C
P=$1
R=${2:-$HOME/.rustup}
for d in "$R"/toolchains/*; do
  [ -x "$d/bin/rustc" ] || continue
  n=$(basename "$d")
  h=$("$d/bin/rustc" -vV 2>/dev/null | sed -n 's/^commit-hash: //p')
  t=$(for x in "$d"/lib/rustlib/*/lib; do [ -d "$x" ] && basename "$(dirname "$x")"; done)
  k=$(sed -n 's/^clippy-preview-.*/clippy/p; s/^rustfmt-preview-.*/rustfmt/p; s/^rust-src$/rust-src/p; s/^llvm-tools-preview-.*/llvm-tools/p' "$d/lib/rustlib/components" 2>/dev/null)
  c=$(printf '%s\n%s\n' "$t" "$k" | grep . | sort | tr '\n' ',' | sed 's/,$//')
  echo "rust $n ${h:-none} ${c:-none}"
done
# go: the bare version, `go version go1.27.1-X:nodwarf5 linux/amd64` (the workstation's build)
# and `go version go1.27.1 linux/amd64` (the node's archive) both print as `go go1.27.1`.
# A node has go and node in /usr/local/bin (the archive pins of provision.sh).
for b in "$P/usr/local/bin/go" "$P/usr/local/go/bin/go" "$P/usr/lib/go/bin/go" "$P/usr/bin/go"; do
  [ -x "$b" ] || continue
  echo "go $("$b" version 2>/dev/null </dev/null | head -n 1 | awk '{ print $3 }' | cut -d- -f1)"
  break
done
for b in "$P/usr/local/bin/node" "$P/usr/bin/node"; do
  [ -x "$b" ] || continue
  echo "node $("$b" --version 2>/dev/null </dev/null | head -n 1)"
  break
done
# C side: the compilers and cross compilers a job resolves through /usr/bin, and the compile cache
# (a node has it in the build user's cargo bin) with the server that runs it (the process
# listening on 4226).
for n in cc gcc clang sccache x86_64-w64-mingw32-gcc musl-gcc; do
  b=$P/usr/bin/$n
  if [ ! -x "$b" ] && [ "$n" = sccache ] && [ -x "$HOME/.cargo/bin/sccache" ]; then b=$HOME/.cargo/bin/sccache; fi
  if [ -x "$b" ]; then
    echo "c $n $("$b" --version 2>/dev/null </dev/null | head -n 1)"
  else
    echo "c $n missing"
  fi
done
s=$(ss -ltnpH 'sport = :4226' 2>/dev/null | sed -n 's/.*pid=\([0-9]*\).*/\1/p' | head -1)
if [ -n "$s" ] && [ -r "/proc/$s/exe" ]; then echo "c sccache-server $(sha256sum "/proc/$s/exe" | cut -c1-16)"; else echo "c sccache-server none"; fi
