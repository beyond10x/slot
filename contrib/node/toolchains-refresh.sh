#!/bin/sh
# toolchains-refresh.sh NODE [SECONDS]: on the workstation, keeps the two toolchain manifests a
# wrapper compares before it sends a job to NODE.
#   <state>/toolchains-local.txt  what toolchains.sh prints here
#   <state>/toolchains-node.txt   what NODE's timer slot-toolchains-manifest wrote there
# <state> is ${XDG_STATE_HOME:-~/.local/state}/slot. NODE is a host of the ssh config
# ${XDG_CONFIG_HOME:-~/.config}/slot/node/ssh_config (the build key); the node's file is read at
# the same path under the same home (TOOLCHAINS_NODE_FILE names another path).
#
# Every SECONDS (60) both files are written again, each through a temporary file of this process
# and a rename: a reader sees a whole manifest or the one before. A run of toolchains.sh that
# fails or prints nothing, a node that does not answer within 60 s, and an empty file or one over
# 1 MiB from the node keep the file that is there, and one line on stderr says which and why; the
# loop goes on. SECONDS 0 writes both once and exits: 0 when both were written, 1 otherwise.
#
# The two files have one name each, so one state directory serves one node. The loop holds a lock
# on <state>/toolchains-refresh.lock, which names its node: a second loop, and a single run for
# another node, exit 2 with a line that names the node of the first. A single run for the same
# node goes on beside the loop.
#
# The local manifest depends on the PATH this script runs with (the tools it finds first). Run it
# with the PATH the builds of this host see: the unit reads one from an environment file.
#
# Exit 2: a usage error or the lock; the unit does not start the script again after it.
#
# toolchains.sh is the copy beside this script. As a service:
# contrib/slot-toolchains-refresh@.service (instance name = NODE).
export LC_ALL=C
node=${1:-}
every=${2:-60}
case $node in
  "" | -* | *[!A-Za-z0-9_.-]*)
    echo "usage: toolchains-refresh.sh NODE [SECONDS]" >&2
    exit 2
    ;;
esac
case $every in
  "" | *[!0-9]*)
    echo "toolchains-refresh.sh: SECONDS must be a whole number, not '$every'" >&2
    exit 2
    ;;
esac
here=$(cd "$(dirname "$0")" && pwd)
S=${XDG_STATE_HOME:-$HOME/.local/state}/slot
C=${XDG_CONFIG_HOME:-$HOME/.config}/slot/node/ssh_config
F=${TOOLCHAINS_NODE_FILE:-$S/toolchains-node.txt}
case $C in
  *\'*)
    echo "toolchains-refresh.sh: the path of the ssh config holds a quote: $C" >&2
    exit 2
    ;;
esac
mkdir -p "$S" || exit 1

# One loop per state directory. The lock is on the file, its text is the node of the holder. No
# child keeps descriptor 9, so the lock ends with this process.
L=$S/toolchains-refresh.lock
exec 9>>"$L" || exit 1
if flock -n 9; then
  printf '%s\n' "$node" >"$L"
else
  held=$(cat "$L" 2>/dev/null)
  if [ "$every" -ne 0 ] || [ "$held" != "$node" ]; then
    echo "toolchains-refresh.sh: another run keeps the manifests of '$held' in $S" >&2
    exit 2
  fi
fi

lt=$S/toolchains-local.txt.tmp.$$
nt=$S/toolchains-node.txt.tmp.$$
trap 'rm -f "$lt" "$nt"' EXIT
trap 'exit 143' TERM
trap 'exit 130' INT

once() {
  st=0
  if sh "$here/toolchains.sh" "" "${RUSTUP_HOME:-$HOME/.rustup}" >"$lt" 9>&- && [ -s "$lt" ]; then
    mv "$lt" "$S/toolchains-local.txt" || st=1
  else
    rm -f "$lt"
    echo "toolchains-refresh.sh: toolchains.sh printed nothing here: toolchains-local.txt kept" >&2
    st=1
  fi
  rm -f "$nt"
  if why=$(timeout 60 rsync -t --timeout=20 --max-size=1m \
    -e "ssh -F '$C' -o BatchMode=yes -o ConnectTimeout=10" "$node:$F" "$nt" 2>&1 9>&-) &&
    [ -s "$nt" ]; then
    mv "$nt" "$S/toolchains-node.txt" || st=1
  else
    rc=$?
    rm -f "$nt"
    why=$(printf '%s\n' "$why" | sed -n '$p')
    echo "toolchains-refresh.sh: no manifest from $node: toolchains-node.txt kept (${why:-exit $rc, no file or an empty one})" >&2
    st=1
  fi
  return $st
}

if [ "$every" -eq 0 ]; then
  once
  exit $?
fi
while :; do
  once
  sleep "$every" 9>&-
done
