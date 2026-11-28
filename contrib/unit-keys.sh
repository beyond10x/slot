#!/bin/sh
# unit-keys.sh <unit> <unit>: exit 0 when two systemd unit files set the same keys to the same
# values, else print the difference and exit 1. Compared: every `Key=value` line, sorted, without
# Description, the generator's X-Cloudspec-* lines and Type=exec (the generator's default; the
# hand-written units start the same program with Type=simple). A duration with a trailing `s`
# equals the bare number of seconds.
set -u
[ $# -eq 2 ] || {
  echo "usage: unit-keys.sh <unit> <unit>" >&2
  exit 2
}
keys() {
  grep -E '^[A-Z][A-Za-z]*=' "$1" |
    grep -v -E '^(Description|X-Cloudspec-[A-Za-z-]*|Type)=' |
    sed -E 's/^(RestartSec|TimeoutStopSec|TimeoutStartSec)=([0-9]+)s$/\1=\2/' |
    sort
}
for f in "$1" "$2"; do
  [ -f "$f" ] || {
    echo "unit-keys.sh: $f: no such file" >&2
    exit 2
  }
done
a=$(keys "$1")
b=$(keys "$2")
[ "$a" = "$b" ] && exit 0
echo "unit-keys.sh: $1 and $2 differ:"
d=$(mktemp -d) || exit 2
printf '%s\n' "$a" > "$d/a"
printf '%s\n' "$b" > "$d/b"
diff "$d/a" "$d/b"
rm -rf "$d"
exit 1
