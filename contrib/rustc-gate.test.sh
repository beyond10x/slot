#!/bin/sh
# rustc-gate.test.sh [gate]: the rustc gate passes jobs, SHIM_OFF, editors and queries; refuses the rest.
set -u
g=${1:-contrib/rustc-gate}
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
mkdir -p "$T/bin"
printf '#!/bin/sh\necho sccache\n' > "$T/bin/sccache"
chmod +x "$T/bin/sccache"
fail=0
check() {
  want=$1
  shift
  got=$(env -u SLOT_JOB -u SLOT_SHIM_OFF -u SLOT_EDITOR PATH="$T/bin:/usr/bin:/bin" "$@" 2>/dev/null; echo "rc$?")
  got=$(echo $got)
  if [ "$got" = "$want" ]; then echo "ok   $* -> $got"; else echo "FAIL $* -> $got, want $want"; fail=1; fi
}
check rc3 sh "$g" rustc --crate-name foo src/lib.rs
check "sccache rc0" env SLOT_JOB=7 sh "$g" rustc --crate-name foo src/lib.rs
check "sccache rc0" env SLOT_SHIM_OFF=1 sh "$g" rustc --crate-name foo src/lib.rs
check "sccache rc0" env SLOT_EDITOR=1 sh "$g" rustc --crate-name foo src/lib.rs
check rc3 env SLOT_EDITOR=yes sh "$g" rustc --crate-name foo src/lib.rs
check "sccache rc0" sh "$g" rustc -vV
check "sccache rc0" sh "$g" rustc - --crate-name ___ --print=file-names --crate-type bin
check "sccache rc0" sh "$g" rustc --print sysroot
check "sccache rc0" env SLOT_LOCAL_REASON="local requested" sh "$g" rustc --crate-name foo src/lib.rs
check rc3 env SLOT_LOCAL_REASON="cache rule" sh "$g" rustc --crate-name foo src/lib.rs
line=$(env -u SLOT_JOB PATH="$T/bin:/usr/bin:/bin" sh "$g" rustc --crate-name foo x.rs 2>&1 >/dev/null)
case $line in *"compile of foo outside a slot job"*"slot run -- "*"SLOT_SHIM_OFF=1"*"SLOT_EDITOR=1"*) echo "ok   refusal line" ;; *) echo "FAIL line: $line"; fail=1 ;; esac
exit $fail
