#!/bin/sh
# Tests of contrib/unit-keys.sh: equal keys pass, a changed or missing key fails, the ignored
# lines and a trailing `s` on a duration make no difference.
set -u
here=$(cd "$(dirname "$0")" && pwd)
t=$(mktemp -d) || exit 1
trap 'rm -rf "$t"' EXIT
fail=0
ok() { echo "ok   $1"; }
bad() {
  echo "FAIL $1"
  fail=1
}
printf '[Unit]\nDescription=a\n\n[Service]\nExecStart=%%h/bin/x serve\nRestartSec=10\nKillMode=mixed\n' > "$t/hand"
printf '[Unit]\nDescription=b\nX-Cloudspec-Source=arch sha256:00 parts.x\n\n[Service]\nType=exec\nKillMode=mixed\nExecStart=%%h/bin/x serve\nRestartSec=10s\n' > "$t/gen"
if sh "$here/unit-keys.sh" "$t/hand" "$t/gen" > /dev/null; then ok "same keys, other order, ignored lines"; else bad "same keys, other order, ignored lines"; fi
sed 's/KillMode=mixed/KillMode=process/' "$t/gen" > "$t/gen2"
if sh "$here/unit-keys.sh" "$t/hand" "$t/gen2" > /dev/null; then bad "a changed value fails"; else ok "a changed value fails"; fi
grep -v RestartSec "$t/gen" > "$t/gen3"
if sh "$here/unit-keys.sh" "$t/hand" "$t/gen3" > /dev/null; then bad "a missing key fails"; else ok "a missing key fails"; fi
sh "$here/unit-keys.sh" "$t/hand" "$t/none" > /dev/null 2>&1
[ $? -eq 2 ] && ok "a missing file exits 2" || bad "a missing file exits 2"
exit $fail
