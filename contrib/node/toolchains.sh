#!/bin/sh
# toolchains.sh <usr-prefix> <rustup-home>: one line per toolchain a build job can resolve to.
#   rust <name> <rustc commit hash> <installed targets, clippy, rustfmt, rust-src and llvm-tools,
#        sorted and comma separated; rust-docs and the manifests are left out>
#   go <version> / node <version>
#   tool <tsc|prettier|esbuild|pnpm|ess|aep|yq|cargo-about|make|helm> <version>: no line when the machine has none,
#        `missing` as the version when the file is there and prints none
#   tool sdc-aep-<version> <16 hex digits of the SHA-256 of ~/.cache/sdc/aep/<version>/…/aep>
#   tool groovy-all-<version> <16 hex digits of the SHA-256 of
#        ~/.sdkman/candidates/groovy/<version>/embeddable/groovy-all-<version>.jar>
#   c <tool> <first line of its --version>, or `c <tool> missing`
# Run with prefix "" on the workstation and on the node: each prints what its own machine has.
# push.sh check compares the rust, go, node and tool lines; the C tools are for reading.
export LC_ALL=C
P=$1
R=${2:-$HOME/.rustup}
for d in "$R"/toolchains/*; do
  [ -x "$d/bin/rustc" ] || continue
  n=$(basename "$d")
  h=$("$d/bin/rustc" -vV 2>/dev/null | sed -n 's/^commit-hash: //p')
  t=$(for x in "$d"/lib/rustlib/*/lib; do [ -n "$(find "$x/" -type f 2>/dev/null | head -n 1)" ] && basename "$(dirname "$x")"; done)
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
# tool: the TypeScript and JavaScript tools the test lanes of the consumers call by name. A node has
# them in /usr/local/bin (the archive pins of provision.sh); the workstation in /usr/bin or the
# user's ~/.local/bin. ess and cargo-about are in the cargo bin on both (the tool pins of
# provision.sh), aep too (on a node built from its release tag, SOURCE_PINS: its release binary
# needs a newer glibc than a node has), yq in /usr/local/bin on a node and in the user's ~/go/bin on
# the workstation.
# The version is the last word of the first line (`Version 6.0.3` prints as 6.0.3). The command
# runs in / without ESS_TOOLCHAIN: the ess dispatcher prints the release a tree requires, not its own.
for n in tsc prettier esbuild pnpm ess aep yq cargo-about make; do
  for b in "$P/usr/local/bin/$n" "$P/usr/bin/$n" "$HOME/.cargo/bin/$n" "$HOME/go/bin/$n" "$HOME/.local/bin/$n"; do
    [ -x "$b" ] || continue
    tv=$(cd / && env -u ESS_TOOLCHAIN -u ESS_TOOLCHAIN_DELEGATED "$b" --version 2>/dev/null </dev/null | head -n 1 | awk '{ print $NF }')
    echo "tool $n ${tv:-missing}"
    break
  done
done
# helm has no `--version`: `helm version --template {{.Version}}` prints `v3.8.1`. A node has it in
# /usr/local/bin (an archive pin). A workstation may have more than one (a package in /usr/bin and
# another one earlier on PATH), and a job runs the one its PATH names first: with the prefix "" the
# helm on PATH is read first. A helm that a relative PATH entry names (a shell prints `./helm` for it,
# or the absolute path) is made absolute: the command runs in /. So this line, alone among the tool
# lines, depends on the caller's PATH: compare it from a shell with the PATH a job gets.
ph=$([ -n "$P" ] || command -v helm 2>/dev/null)
case $ph in /* | "") ;; *) ph=$(pwd)/$ph ;; esac
for b in "$ph" "$P/usr/local/bin/helm" "$P/usr/bin/helm" "$HOME/.local/bin/helm"; do
  [ -n "$b" ] && [ -x "$b" ] || continue
  hv=$(cd / && "$b" version --template '{{.Version}}' 2>/dev/null </dev/null | head -n 1)
  echo "tool helm ${hv:-missing}"
  break
done
# The aep binary of runbook's driver pin, one file per version in the user's cache
# (~/.cache/sdc/aep/<version>/x86_64-unknown-linux-gnu/aep): `tool sdc-aep-<version> <the first 16
# hex digits of the file's SHA-256>`. The driver compares the whole digest with its pin, so the
# digest, not the version the file prints, is what must agree. The file is not run: the release
# binary asks for a newer glibc than a node has and prints a loader warning.
for f in "$HOME"/.cache/sdc/aep/*/x86_64-unknown-linux-gnu/aep; do
  [ -f "$f" ] || continue
  echo "tool sdc-aep-$(basename "$(dirname "$(dirname "$f")")") $(sha256sum "$f" | cut -c1-16)"
done
# The Groovy distributions under the path sdkman uses (~/.sdkman/candidates/groovy/<version>) that
# carry a groovy-all jar: `tool groovy-all-<version> <the first 16 hex digits of the jar's
# SHA-256>`. A version directory that is a symbolic link counts (sdkman's local install makes one).
# A directory without a jar of its own name prints no line: sdkman's `current` link, and a release
# that ships no groovy-all jar.
for d in "$HOME"/.sdkman/candidates/groovy/*; do
  [ -d "$d" ] || continue
  f=$d/embeddable/groovy-all-$(basename "$d").jar
  [ -f "$f" ] || continue
  echo "tool groovy-all-$(basename "$d") $(sha256sum "$f" | cut -c1-16)"
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
