#!/bin/sh
# push.sh: the workstation half of a node's parity check. A node starts as a bare image and
# `provision.sh apply` installs everything its jobs use (packages, rust toolchains, pinned tools,
# the slot release); nothing is copied from the workstation to a node. Runs on the workstation.
#
#   push.sh check CONF   compare the workstation's rust, go, node and tool lines of toolchains.sh and the
#                        release of each pinned tool (`provision.sh pins`) with the node's, from
#                        one slot job on NODE_HOST run in the checkout NODE_RUN_DIR; print one
#                        `drift: push: ...` line per difference and `<n> drift`; exit 0 or 1
#   push.sh compare HERE NODE   the comparison of check, over two digest files
#
# This is a version parity report, not an integrity check: the job runs as the build user, and a
# job that runs as that user can change what the node reports. Control characters of the node's
# output are dropped before it is printed.
#
# CONF is the node config file of provision.sh (node.conf.example).
set -u
here=$(cd "$(dirname "$0")" && pwd)
. "$here/conf.sh"

# Compares the digest lines of the workstation (file $1) with the node's (file $2). The rust, go,
# node and tool lines count both ways, except a rust toolchain the node has and the workstation lacks
# (no job resolves to it: the shim compares the active toolchain's line); the C tools differ
# between machines and are not compared. A
# `pin <name> <version>` line is the release of a tool provision.sh installs (`provision.sh
# pins`): the node's must be that release, and the workstation's is not compared.
compare() {
  # The node's lines come from a job that runs as the build user: control characters (an escape
  # sequence that moves or rewrites the terminal) are dropped before anything is printed.
  nf=$2.c
  tr -d '\000-\010\013-\037\177' <"$2" >"$nf"
  nh=$(sed -n 's/^host //p' "$nf")
  hh=$(sed -n 's/^host //p' "$1")
  if [ -z "$nh" ] || [ "$nh" = "$hh" ]; then
    echo "push: the digest job did not run on the node (host '${nh:-none}')" >&2
    rm -f "$nf"
    return 2
  fi
  grep '^rust \|^go \|^node \|^tool ' "$1" | LC_ALL=C sort >"$1.s"
  grep '^rust \|^go \|^node \|^tool ' "$nf" | LC_ALL=C sort >"$2.s"
  {
    sh "$here/provision.sh" pins | while read -r pb pv; do
      got=$(sed -n "s/^pin $pb //p" "$nf" | head -n 1)
      [ "$got" = "$pv" ] || echo "drift: push: pin $pb: node has '$([ -n "$got" ] && echo "$pb $got" || echo missing)', want $pb $pv"
    done
    wn=" $(sed -n 's/^rust \([^ ]*\) .*/\1/p' "$1" | tr '\n' ' ')"
    LC_ALL=C comm -3 "$1.s" "$2.s" | while IFS= read -r l; do
      case $l in
        "	rust "*)
          rn=${l#	rust }
          rn=${rn%% *}
          case $wn in *" $rn "*) echo "drift: push: only on the node: ${l#	}" ;; esac
          ;;
        "	"*) echo "drift: push: only on the node: ${l#	}" ;;
        *) echo "drift: push: workstation has: $l" ;;
      esac
    done
  } >"$2.d"
  rm -f "$1.s" "$2.s" "$nf"
  cat "$2.d"
  n=$(grep -c . "$2.d")
  rm -f "$2.d"
  echo "$n drift"
  [ "$n" -eq 0 ]
}

mode=${1:-}
cf=${2:-}
case $mode in
  check) conf_read "$cf" BUILD_USER BUILD_HOME SLOT_VERSION NODE_SSH NODE_HOST NODE_RUN_DIR BUILD_UID BUILD_KEY || exit 2 ;;
  compare)
    compare "$2" "$3"
    exit
    ;;
  *)
    sed -n '2,/^set -u/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' >&2
    exit 2
    ;;
esac
H=$BUILD_HOME

# check: the same digest script runs here and in one job on the node.
digest=$(
  cat "$here/toolchains.sh"
  cat <<EOF
echo "host \$(hostname)"
cd "\$HOME/.cargo/bin" 2>/dev/null
for b in $(sh "$here/provision.sh" pins | cut -d' ' -f1 | tr '\n' ' '); do
  v=\$(env -u ESS_TOOLCHAIN -u ESS_TOOLCHAIN_DELEGATED "./\$b" --version 2>/dev/null </dev/null | head -n 1 | cut -d' ' -f2)
  echo "pin \$b \${v:-missing}"
done
EOF
)
here_out=$(mktemp)
node_out=$(mktemp)
trap 'rm -f "$here_out" "$node_out"' EXIT
echo "push: nothing is copied to the node; it installs everything itself (provision.sh apply)"
sh -c "$digest" sh "" "$HOME/.rustup" >"$here_out" 2>/dev/null
(cd "$NODE_RUN_DIR" && SLOT_HOST=$NODE_HOST SLOT_SSH=$NODE_SSH SLOT_REMOTE=/usr/local/bin/slot slot sh -c "$digest" sh "" "$H/.rustup") >"$node_out" 2>/dev/null
compare "$here_out" "$node_out"
