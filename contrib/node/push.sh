#!/bin/sh
# push.sh: the workstation half of a node's provisioning. A node's jobs see the workstation's
# /usr, a few /etc entries, its rust toolchains and cargo tools; they live on the node's instance
# store, which is empty after every stop and start. Runs on the workstation as the build user.
#
#   push.sh apply CONF   copy them to the node over NODE_SSH to PUSH_HOST (the node's setup
#                        login, open only while `provision.sh apply CONF --open` let it in)
#   push.sh check CONF   compare digests of the workstation's copies with the node's, from one
#                        slot job on NODE_HOST run in the checkout NODE_RUN_DIR; print one
#                        `drift: push: ...` line per difference and `<n> drift`; exit 0 or 1
#   push.sh compare HERE NODE   the comparison of check, over two digest files
#
# CONF is the node config file of provision.sh (node.conf.example).
set -u
here=$(cd "$(dirname "$0")" && pwd)
. "$here/conf.sh"

# Compares the digest lines of the workstation (file $1) with the node's (file $2). Every line
# counts both ways, except a `copy <dir>/<file> <size>` line only on the node: a job there may
# add files to a copied directory such as a cache, and push copies without deleting.
compare() {
  nh=$(sed -n 's/^host //p' "$2")
  hh=$(sed -n 's/^host //p' "$1")
  if [ -z "$nh" ] || [ "$nh" = "$hh" ]; then
    echo "push: the digest job did not run on the node (host '${nh:-none}')" >&2
    return 2
  fi
  grep -v '^host ' "$1" | LC_ALL=C sort >"$1.s"
  grep -v '^host ' "$2" | LC_ALL=C sort >"$2.s"
  LC_ALL=C comm -3 "$1.s" "$2.s" | while IFS= read -r l; do
    case $l in
      "	copy "*) ;;
      "	"*) echo "drift: push: only on the node: ${l#	}" ;;
      *) echo "drift: push: workstation has: $l" ;;
    esac
  done >"$2.d"
  rm -f "$1.s" "$2.s"
  cat "$2.d"
  n=$(grep -c . "$2.d")
  rm -f "$2.d"
  echo "$n drift"
  [ "$n" -eq 0 ]
}

mode=${1:-}
cf=${2:-}
case $mode in
  apply) conf_read "$cf" BUILD_USER BUILD_HOME SLOT_VERSION NODE_SSH PUSH_HOST BUILD_UID BUILD_KEY || exit 2 ;;
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
ETC_LIST='ld.so.cache fonts localtime locale.conf ssl ca-certificates nsswitch.conf ld.so.conf ld.so.conf.d mime.types protocols services os-release'
toolchains() {
  if [ -n "$RUST_TOOLCHAINS" ]; then
    echo "$RUST_TOOLCHAINS"
  else
    for d in "$HOME"/.rustup/toolchains/*; do [ -x "$d/bin/rustc" ] && basename "$d"; done
  fi
}
skip() {
  for s in $SKIP_BINS; do [ "$s" = "$1" ] && return 0; done
  return 1
}

# rsync from the default PATH (a wrapper earlier on PATH may drop times); exit 23 and 24: some
# files could not be read (root-only files under /usr) or vanished;
# the toolchain manifest leaves the unreadable ones out too.
rs() {
  command -p rsync "$@"
  r=$?
  case $r in 0 | 23 | 24) return 0 ;; *)
    echo "push: rsync $* exited $r" >&2
    return 1
    ;;
  esac
}

if [ "$mode" = apply ]; then
  # One ssh control connection of this run's own, ended on exit: a connection shared through
  # the ssh config would stay logged in after the setup key leaves authorized_keys.
  cdir=$(mktemp -d) || exit 1
  NODE_SSH="$NODE_SSH -o ControlMaster=auto -o ControlPersist=60 -o ControlPath=$cdir/m"
  trap '$NODE_SSH -O exit "$PUSH_HOST" >/dev/null 2>&1; rm -rf "$cdir"' EXIT
  dst=$PUSH_HOST:$STORE
  fails=0
  t0=$(date +%s)
  # /usr: the top level first (links such as sbin and lib64, empty directories), then one
  # stream per top-level directory, six at a time.
  rs -a --numeric-ids --mkpath -e "$NODE_SSH" --include='/*' --exclude='/*/**' /usr/ "$dst/rootfs/usr/" || fails=$((fails + 1))
  n=0
  for d in /usr/*/; do
    d=${d%/}
    [ -L "$d" ] && continue
    rs -aH --numeric-ids --delete -e "$NODE_SSH" "$d/" "$dst/rootfs/usr/${d#/usr/}/" &
    n=$((n + 1))
    [ $((n % 6)) -eq 0 ] && wait
  done
  wait
  for e in $ETC_LIST $ETC_EXTRA; do
    [ -e "/etc/$e" ] || continue
    if [ -d "/etc/$e" ] && [ ! -L "/etc/$e" ]; then
      rs -a --delete --mkpath -e "$NODE_SSH" "/etc/$e/" "$dst/rootfs/etc/$e/" || fails=$((fails + 1))
    else
      rs -Lt --mkpath -e "$NODE_SSH" "/etc/$e" "$dst/rootfs/etc/" || fails=$((fails + 1))
    fi
  done
  for t in $(toolchains); do
    rs -a --delete --mkpath -e "$NODE_SSH" "$HOME/.rustup/toolchains/$t/" "$dst/rustup/toolchains/$t/" || fails=$((fails + 1))
  done
  def=$(sed -n 's/^default_toolchain = //p' "$HOME/.rustup/settings.toml" 2>/dev/null)
  printf 'version = "12"\ndefault_toolchain = %s\nprofile = "default"\n\n[overrides]\n' "${def:-\"stable\"}" |
    $NODE_SSH "$PUSH_HOST" "cat > $STORE/rustup/settings.toml" || fails=$((fails + 1))
  list=$(mktemp)
  (cd "$HOME/.cargo/bin" && for b in *; do [ -f "$b" ] && ! skip "$b" && echo "$b"; done) >"$list"
  rs -aL --files-from="$list" -e "$NODE_SSH" "$HOME/.cargo/bin/" "$dst/cargo/bin/" || fails=$((fails + 1))
  rm -f "$list"
  for p in $HOME_COPIES; do
    [ -e "$HOME/$p" ] || continue
    rs -a --mkpath -e "$NODE_SSH" "$HOME/$p/" "$PUSH_HOST:$H/$p/" || fails=$((fails + 1))
  done
  echo "push: done in $(($(date +%s) - t0)) s, $fails failed step(s)"
  [ $fails -eq 0 ]
  exit
fi

# check: the same digest script runs here and in one job on the node; a job there sees the
# workstation copies as its /usr, /etc entries, ~/.rustup and ~/.cargo/bin.
digest=$(
  cat "$here/toolchains.sh"
  cat <<EOF
echo "host \$(hostname)"
cd "\$HOME/.cargo/bin" 2>/dev/null && for b in *; do
  [ -f "\$b" ] || continue
  case " $SKIP_BINS " in *" \$b "*) continue ;; esac
  echo "bin \$b \$(stat -Lc %s "\$b")"
done
for p in $HOME_COPIES; do
  [ -d "\$HOME/\$p" ] || { echo "copy \$p missing"; continue; }
  (cd "\$HOME/\$p" && find . -type f -printf "copy \$p/%P %s\n")
done
EOF
)
here_out=$(mktemp)
node_out=$(mktemp)
trap 'rm -f "$here_out" "$node_out"' EXIT
sh -c "$digest" sh "" "$HOME/.rustup" >"$here_out" 2>/dev/null
(cd "$NODE_RUN_DIR" && SLOT_HOST=$NODE_HOST SLOT_SSH=$NODE_SSH SLOT_REMOTE=/usr/local/bin/slot slot sh -c "$digest" sh "" "$H/.rustup") >"$node_out" 2>/dev/null
compare "$here_out" "$node_out"
