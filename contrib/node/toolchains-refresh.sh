#!/bin/sh
# toolchains-refresh.sh NODE [SECONDS]: on the workstation, keeps the two toolchain manifests a
# wrapper compares before it sends a job to NODE.
#   <state>/toolchains-local.txt  what toolchains.sh prints here
#   <state>/toolchains-node.txt   what NODE's timer slot-toolchains-manifest wrote there
# <state> is ${XDG_STATE_HOME:-~/.local/state}/slot. NODE is a host of the ssh config
# ${XDG_CONFIG_HOME:-~/.config}/slot/node/ssh_config (the build key); the node's file is read at
# the same path under the same home (TOOLCHAINS_NODE_FILE names another path).
#
# Every SECONDS (60) both files are written again, each through a temporary file and a rename: a
# reader sees a whole manifest or the one before. A run of toolchains.sh that fails, a node that
# does not answer and an empty file from the node keep the file that is there, and one line on
# stderr says which; the loop goes on. SECONDS 0 writes both once and exits: 0 when both were
# written, 1 otherwise.
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
mkdir -p "$S" || exit 1

once() {
  st=0
  if sh "$here/toolchains.sh" "" "${RUSTUP_HOME:-$HOME/.rustup}" >"$S/toolchains-local.txt.tmp" &&
    [ -s "$S/toolchains-local.txt.tmp" ]; then
    mv "$S/toolchains-local.txt.tmp" "$S/toolchains-local.txt" || st=1
  else
    rm -f "$S/toolchains-local.txt.tmp"
    echo "toolchains-refresh.sh: toolchains.sh printed nothing here: toolchains-local.txt kept" >&2
    st=1
  fi
  rm -f "$S/toolchains-node.txt.tmp"
  if rsync -t -e "ssh -F $C" "$node:$F" "$S/toolchains-node.txt.tmp" 2>/dev/null &&
    [ -s "$S/toolchains-node.txt.tmp" ]; then
    mv "$S/toolchains-node.txt.tmp" "$S/toolchains-node.txt" || st=1
  else
    rm -f "$S/toolchains-node.txt.tmp"
    echo "toolchains-refresh.sh: no manifest from $node: toolchains-node.txt kept" >&2
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
  sleep "$every"
done
