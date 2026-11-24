#!/bin/sh
# Routing test of contrib/slot-shim: which cargo, go and make calls go through slot. Stubs stand in
# for the real tools and for slot; nothing is built. Run from the repository root.
set -u
shim=${1:-contrib/slot-shim}
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
mkdir -p "$T/shim" "$T/bin" "$T/work"
cp "$shim" "$T/shim/slot-shim"
for t in cargo go make; do
  ln -s slot-shim "$T/shim/$t"
  printf '#!/bin/sh\necho direct\n' > "$T/bin/$t"
  chmod +x "$T/bin/$t"
done
printf '#!/bin/sh\necho pool\n' > "$T/bin/slot"
chmod +x "$T/bin/slot"
fail=0
check() {
  want=$1
  shift
  got=$(cd "$T/work" && env -u SLOT_JOB -u SLOT_SHIM_OFF PATH="$T/shim:$T/bin:/usr/bin:/bin" "$T/shim/$@")
  if [ "$got" = "$want" ]; then
    echo "ok   $* -> $got"
  else
    echo "FAIL $* -> $got, want $want"
    fail=1
  fi
}
check pool cargo build
check pool cargo test --workspace
check pool cargo nextest run
check pool cargo run -p xtask
check pool cargo xtask render-bundle
check pool cargo +1.99 run -p x
check pool cargo --color never run
check pool cargo llvm-cov
check pool go test ./...
check pool make all
check direct cargo fmt
check direct cargo metadata --format-version 1
check direct cargo tree
check direct cargo --version
check direct cargo clean
check direct cargo mutants --list
check direct cargo update
check direct go env
got=$(cd "$T/work" && SLOT_JOB=1 PATH="$T/shim:$T/bin:/usr/bin:/bin" "$T/shim/cargo" build)
[ "$got" = direct ] && echo "ok   cargo build inside a job -> direct" || { echo "FAIL cargo build inside a job -> $got"; fail=1; }
exit $fail
