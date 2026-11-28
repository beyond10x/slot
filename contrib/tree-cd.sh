# tree-cd.sh: sourced by contrib/slot-wrapper with the job's arguments set ("$@").
# A job whose --manifest-path names a Cargo.toml outside the working directory's tree runs from
# that manifest's tree instead: the build node syncs (and returns) the tree of the working
# directory only, so the manifest's tree must be that tree.
slot_tree_cd() {
  m=
  take=
  for a in "$@"; do
    if [ -n "$take" ]; then m=$a; break; fi
    case $a in
      --manifest-path) take=1 ;;
      --manifest-path=*) m=${a#--manifest-path=}; break ;;
      --) ;;
    esac
  done
  [ -n "$m" ] || return 0
  case $m in /*) ;; *) return 0 ;; esac
  [ -f "$m" ] || return 0
  here=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
  case $m in "$here"/*) return 0 ;; esac
  d=$(dirname "$m")
  t=$(git -C "$d" rev-parse --show-toplevel 2>/dev/null || echo "$d")
  cd "$t" || return 0
  echo "slot: building in $t (the tree of --manifest-path)" >&2
}
slot_tree_cd "$@"
