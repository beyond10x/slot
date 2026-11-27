#!/bin/sh
# Proves that a file names no path of a home directory: no `/home/` and no `$HOME/` of the
# caller (a home of `/` or an empty HOME adds nothing). A release binary holds the source
# paths of its panic locations; built without a remap, they name the build machine's home.
#
#   no-home-path.sh FILE   prints `no home path: FILE` and exits 0, or says on standard
#                          error which paths it found and exits 1
set -eu

bin=${1:?usage: no-home-path.sh FILE}
LC_ALL=C
export LC_ALL

refuse() {
  echo "no-home-path: $bin $1" >&2
  exit 1
}

[ -f "$bin" ] || refuse "is not a file"

home=${HOME:-}
home=${home%/}
if [ -n "$home" ]; then
  set -- -e /home/ -e "$home/"
else
  set -- -e /home/
fi

if grep -aqF "$@" "$bin"; then
  hits=$(grep -aoF "$@" "$bin" | wc -l | tr -d ' ')
  shown=$(grep -aoE '/[^[:space:][:cntrl:]]{0,120}' "$bin" | grep -F "$@" | sort -u | head -n 10)
  refuse "names a home directory $hits times; the first paths:
$shown"
fi
echo "no home path: $bin"
