#!/bin/sh
# toolchains.sh <usr-prefix> <rustup-home>: one line per toolchain a build job can resolve to.
#   rust <name> <rustc commit hash> <installed targets, clippy, rustfmt, rust-src and llvm-tools,
#        sorted and comma separated; rust-docs and the manifests are left out>
#   go <version> / node <version>
#   goroot <what `go env GOROOT` of that go answers>: no line without go
#   tool <tsc|prettier|esbuild|pnpm|yarn|ess|aep|yq|cargo-about|make|musl|helm> <version>: no line
#        when the machine has none, `missing` as the version when the file is there and prints none
#   tool dash <release>: of the tree a node builds, else of the machine's package `dash`
#   tool firefox <release>: the last word of `firefox --version`
#   tool sdc-aep-<version> <16 hex digits of the SHA-256 of ~/.cache/sdc/aep/<version>/…/aep>
#   tool groovy-all-<version> <16 hex digits of the SHA-256 of
#        ~/.sdkman/candidates/groovy/<version>/embeddable/groovy-all-<version>.jar>
#   tool <erlang|elixir|gleam|odin> <version>: the OTP release (29.0.6), `<release>-otp-<the OTP
#        major it was compiled with>`, the release, and the last word of `odin version`
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
  # goroot: the directory that go takes its standard library from, which a build can carry in
  # its output. A word of its own, so that a reader of the lines that begin with `go ` and a
  # space sees the one line as before. No line for an answer that is no absolute path.
  gr=$("$b" env GOROOT 2>/dev/null </dev/null | head -n 1)
  case $gr in /*) echo "goroot $gr" ;; esac
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
# And with YARN_IGNORE_PATH=1: yarn 1 prints the release a `yarn-path` of ~/.yarnrc names, not
# its own.
for n in tsc prettier esbuild pnpm yarn ess aep yq cargo-about make; do
  for b in "$P/usr/local/bin/$n" "$P/usr/bin/$n" "$HOME/.cargo/bin/$n" "$HOME/go/bin/$n" "$HOME/.local/bin/$n"; do
    [ -x "$b" ] || continue
    tv=$(cd / && env -u ESS_TOOLCHAIN -u ESS_TOOLCHAIN_DELEGATED YARN_IGNORE_PATH=1 "$b" --version 2>/dev/null </dev/null | head -n 1 | awk '{ print $NF }')
    echo "tool $n ${tv:-missing}"
    break
  done
done
# musl: `musl-gcc --version` is the version of gcc, so the release is read from musl's loader,
# libc.so in the directory of the specs file that the wrapper names (`-specs "<dir>/musl-gcc.specs"`
# in its text): run without arguments it prints `Version <release>` on standard error. A node has
# the wrapper in /usr/local/bin (the musl pin of provision.sh, <dir> /usr/local/musl-<release>/lib),
# the workstation in /usr/bin (<dir> /usr/lib/musl/lib).
for b in "$P/usr/local/bin/musl-gcc" "$P/usr/bin/musl-gcc"; do
  [ -x "$b" ] || continue
  ml=$(sed -n 's/.* -specs "\(\/[^"]*\)\/musl-gcc\.specs".*/\1/p' "$b" 2>/dev/null | head -n 1)
  mv=
  if [ -n "$ml" ] && [ -x "$P$ml/libc.so" ]; then mv=$(cd / && "$P$ml/libc.so" 2>&1 </dev/null | sed -n 's/^Version //p' | head -n 1); fi
  echo "tool musl ${mv:-missing}"
  break
done
# dash prints no version. A node has it as a link in /usr/local/bin to
# /usr/local/dash-<release>/bin/dash (the dash pin of provision.sh), a tree that carries the
# SHA-256 of its source tarball: the release is the one of the directory, `missing` without that
# file. Any other dash is a package's: the release is what the machine's package manager names for
# the package `dash` (pacman, dpkg or rpm), without the package revision.
for b in "$P/usr/local/bin/dash" "$P/usr/bin/dash"; do
  dv=
  dl=$(readlink "$b" 2>/dev/null)
  case $dl in /usr/local/dash-*/bin/dash) ;; *) dl= ;; esac
  if [ -n "$dl" ]; then [ -x "$P$dl" ] || continue; else [ -x "$b" ] || continue; fi
  case $dl in
    /usr/local/dash-*/bin/dash)
      dd=${dl%/bin/dash}
      [ -s "$P$dd/.slot-sha256" ] && dv=${dd#/usr/local/dash-}
      ;;
    *)
      dv=$(pacman -Q dash 2>/dev/null </dev/null | awk '{ print $2 }')
      [ -n "$dv" ] || dv=$(dpkg-query -W -f '${Version}\n' dash 2>/dev/null </dev/null)
      [ -n "$dv" ] || dv=$(rpm -q --qf '%{VERSION}\n' dash 2>/dev/null </dev/null) || dv=
      dv=$(echo "$dv" | head -n 1 | sed 's/^[0-9]*://; s/-.*//')
      ;;
  esac
  echo "tool dash ${dv:-missing}"
  break
done
# firefox: the browser a consumer's tests start by name, headless. A node has it in /usr/local/bin
# when its config names it in EXTRA_TOOLS (provision.sh), the workstation in
# /usr/bin. `firefox --version` prints `Mozilla Firefox <release>` and opens no window. The node's
# link names /usr/local/firefox-<release>/firefox, read under the prefix.
for b in "$P/usr/local/bin/firefox" "$P/usr/bin/firefox"; do
  fl=$(readlink "$b" 2>/dev/null)
  case $fl in /usr/local/firefox-*/firefox) b=$P$fl ;; esac
  [ -x "$b" ] || continue
  fv=$(cd / && MOZ_HEADLESS=1 "$b" --version 2>/dev/null </dev/null | head -n 1 | awk '{ print $NF }')
  echo "tool firefox ${fv:-missing}"
  break
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
# Erlang/OTP, Elixir, Gleam and Odin (EXTRA_TOOLS of provision.sh). A node has them in
# /usr/local/bin. A workstation has erl in /usr/bin, and the others where its PATH names them: like
# helm, with the prefix "" the one PATH names first is read, so these four lines depend on the
# caller's PATH as well. No tool here has a `--version` that is its release on the first line:
#   erlang  erl has none. The OTP release is the text of releases/<major>/OTP_VERSION under the
#           root of the system that runs, which erl prints here (29.0.6).
#   elixir  `--version` prints the line of the Erlang system first, then `Elixir 1.20.4 (compiled
#           with Erlang/OTP 29)`: the line is `1.20.4-otp-29`, the release and the OTP major of the
#           precompiled archive. It runs `erl` from the caller's PATH; without one: `missing`.
#   gleam   the last word of `gleam --version`.
#   odin    the last word of `odin version` (`dev-2026-09-nightly:a2fb372`: release and commit).
for n in erl elixir gleam odin; do
  pb=$([ -n "$P" ] || command -v "$n" 2>/dev/null)
  case $pb in /* | "") ;; *) pb=$(pwd)/$pb ;; esac
  for b in "$pb" "$P/usr/local/bin/$n" "$P/usr/bin/$n" "$HOME/.local/bin/$n"; do
    [ -n "$b" ] && [ -x "$b" ] || continue
    case $n in
      erl) bv=$(cd / && "$b" -noshell -eval '{ok,V}=file:read_file(filename:join([code:root_dir(),"releases",erlang:system_info(otp_release),"OTP_VERSION"])), io:put_chars(V), halt().' 2>/dev/null </dev/null | head -n 1) ;;
      elixir) bv=$(cd / && "$b" --version 2>/dev/null </dev/null | sed -n 's/^Elixir \([^ ]*\) (compiled with Erlang\/OTP \([0-9]*\)).*/\1-otp-\2/p' | head -n 1) ;;
      odin) bv=$(cd / && "$b" version 2>/dev/null </dev/null | head -n 1 | awk '{ print $NF }') ;;
      *) bv=$(cd / && "$b" --version 2>/dev/null </dev/null | head -n 1 | awk '{ print $NF }') ;;
    esac
    [ "$n" != erl ] || n=erlang
    echo "tool $n ${bv:-missing}"
    break
  done
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
# listening on 4226). musl-gcc: a node has it in /usr/local/bin (the musl pin).
for n in cc gcc clang sccache x86_64-w64-mingw32-gcc musl-gcc; do
  b=$P/usr/bin/$n
  if [ ! -x "$b" ] && [ "$n" = sccache ] && [ -x "$HOME/.cargo/bin/sccache" ]; then b=$HOME/.cargo/bin/sccache; fi
  if [ ! -x "$b" ] && [ "$n" = musl-gcc ] && [ -x "$P/usr/local/bin/musl-gcc" ]; then b=$P/usr/local/bin/musl-gcc; fi
  if [ -x "$b" ]; then
    echo "c $n $("$b" --version 2>/dev/null </dev/null | head -n 1)"
  else
    echo "c $n missing"
  fi
done
s=$(ss -ltnpH 'sport = :4226' 2>/dev/null | sed -n 's/.*pid=\([0-9]*\).*/\1/p' | head -1)
if [ -n "$s" ] && [ -r "/proc/$s/exe" ]; then echo "c sccache-server $(sha256sum "/proc/$s/exe" | cut -c1-16)"; else echo "c sccache-server none"; fi
