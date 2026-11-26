#!/bin/sh
# The red check of the story "a crate built in one worktree is served from the compile cache in
# the next" (.engineering/planning/story). Not part of `task check`: it needs a live compile cache
# and builds through the pool.
#
# Is a crate that was compiled once served from the shared compile cache when the same
# crate is built a second time for another target directory, from another tree, or by another kind
# of job?
#
# usage: cache-key-probe.sh <mode> <package spec> <tree A> [<tree B>]
#   mode tree     build in tree A, then in tree B, each into its own default target directory
#   mode var      build twice in tree A, the target directory named by CARGO_TARGET_DIR (two values)
#   mode flag     build twice in tree A, the target directory named by --target-dir (two values),
#                 the environment unchanged
#   mode runner   like flag, and the second build has CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER
#                 set, as a test lane has
#   mode profile  build twice in tree A into ONE target directory, in the profiles PROBE_PROFILES
#                 names (default "wasm-gate wasm-lint": two profiles with the same settings), so
#                 only the name of the profile directory differs
#   mode jobkind  like flag, and the second build is cargo inside a generic job
#                 (`slot sh -c 'cargo build …'`, as a gate run through `slot task …` starts cargo)
# PROBE_CARGO_ARGS in the environment: more cargo arguments for both builds, for example
#   "--target wasm32-unknown-unknown"
#
# Every run compiles with a `--cfg` name of its own (RUSTFLAGS), so the first build cannot be in the
# cache: its time for the package is the time of a real compile under the load of this minute. The
# second build is "served" when its time for the same package is at most a third of that.
# exit 0: served.  exit 1: compiled again.  exit 2: the probe itself did not work.
#
# Runs without `slot` in front: each `cargo` goes to the pool by itself, each read is `slot sh -c`.
# Each job is a build of one small package and ends within the pool's 10 minutes.
set -u
mode=$1
pkg=$2
a=$3
b=${4:-$3}
more=${PROBE_CARGO_ARGS:-}
key="slot_probe_$(date +%s)_$$"
# Logs and timing reports of this run; the path is printed when the probe itself fails.
out=$(mktemp -d "${TMPDIR:-/tmp}/slot-cache-key-probe.XXXXXX") || exit 2

# units <timing.html>: "<duration s>\t<name> <version>\t<target>" per unit of a cargo --timings
# report (needs jq).
units() {
  awk '/^const UNIT_DATA = /{sub(/^const UNIT_DATA = /,""); on=1} on{print} on&&/^\];/{exit}' "$1" | sed '$s/;$//' |
    jq -r '.[] | [(.duration|tostring), (.name + " " + .version), (.target // "")] | @tsv'
}
name=${pkg%@*}
ver=
case $pkg in *@*) ver=${pkg#*@} ;; esac

# one <n> <tree> <target dir relative to the tree> <env assignment, "" or GENERIC> [cargo args…]
one() {
  n=$1; tree=$2; dir=$3; envset=$4; shift 4
  cd "$tree" || exit 2
  if [ "$envset" = GENERIC ]; then
    slot sh -c "RUSTFLAGS='--cfg $key' exec cargo build -v --locked --timings -p $pkg $more $*" > "$out/build$n.log" 2>&1
  else
    # shellcheck disable=SC2086
    env RUSTFLAGS="--cfg $key" $envset cargo build -v --locked --timings -p "$pkg" $more "$@" > "$out/build$n.log" 2>&1
  fi || { echo "probe: build $n failed, see $out/build$n.log"; exit 2; }
  grep -q -- "--cfg $key" "$out/build$n.log" || { echo "probe: build $n did not get the --cfg of this run"; exit 2; }
  slot sh -c 'cat "$(ls -t '"$dir"'/cargo-timings/cargo-timing-*.html | head -1)"' > "$out/timing$n.html" 2> "$out/timing$n.err"
  units "$out/timing$n.html" > "$out/units$n.tsv"
  awk -F'\t' -v n="$name" -v v="$ver" '$3=="" { split($2, p, " "); if (p[1]==n && (v=="" || p[2]==v)) print $1 }' "$out/units$n.tsv" | sort -rn | head -1
}

case $mode in
  tree)
    [ "$a" != "$b" ] || { echo "probe: mode tree needs two trees"; exit 2; }
    t1=$(one 1 "$a" target "") || { echo "$t1"; exit 2; }
    t2=$(one 2 "$b" target "") || { echo "$t2"; exit 2; }
    what="tree $(basename "$a") then tree $(basename "$b"), default target directories" ;;
  var)
    t1=$(one 1 "$a" "target/$key-1" "CARGO_TARGET_DIR=target/$key-1") || { echo "$t1"; exit 2; }
    t2=$(one 2 "$a" "target/$key-2" "CARGO_TARGET_DIR=target/$key-2") || { echo "$t2"; exit 2; }
    what="one tree, CARGO_TARGET_DIR=target/…-1 then …-2" ;;
  flag)
    t1=$(one 1 "$a" "target/$key-1" "" --target-dir "target/$key-1") || { echo "$t1"; exit 2; }
    t2=$(one 2 "$a" "target/$key-2" "" --target-dir "target/$key-2") || { echo "$t2"; exit 2; }
    what="one tree, --target-dir target/…-1 then …-2, environment unchanged" ;;
  runner)
    t1=$(one 1 "$a" "target/$key-1" "" --target-dir "target/$key-1") || { echo "$t1"; exit 2; }
    t2=$(one 2 "$a" "target/$key-2" "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner" --target-dir "target/$key-2") || { echo "$t2"; exit 2; }
    what="one tree, --target-dir target/…-1 then …-2, the second with CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER set" ;;
  jobkind)
    t1=$(one 1 "$a" "target/$key-1" "" --target-dir "target/$key-1") || { echo "$t1"; exit 2; }
    t2=$(one 2 "$a" "target/$key-2" GENERIC --target-dir "target/$key-2") || { echo "$t2"; exit 2; }
    what="one tree, --target-dir target/…-1 then …-2, the first a cargo job, the second cargo inside a generic job" ;;
  profile)
    set -- ${PROBE_PROFILES:-wasm-gate wasm-lint}
    p1=$1; p2=$2
    t1=$(one 1 "$a" "target/$key-1" "" --target-dir "target/$key-1" --profile "$p1") || { echo "$t1"; exit 2; }
    t2=$(one 2 "$a" "target/$key-1" "" --target-dir "target/$key-1" --profile "$p2") || { echo "$t2"; exit 2; }
    what="one tree, one target directory, --profile $p1 then --profile $p2" ;;
  *) echo "probe: mode is tree, var, flag, runner, jobkind or profile"; exit 2 ;;
esac
case $mode in var | flag | runner | jobkind | profile) (cd "$a" && slot sh -c "rm -rf target/$key-1 target/$key-2" > /dev/null 2>&1) ;; esac
[ -n "$t1" ] && [ -n "$t2" ] || { echo "probe: no time for $pkg in the timing report, see $out"; exit 2; }
if awk -v x="$t1" -v y="$t2" 'BEGIN { exit !(y * 3 <= x) }'; then
  echo "PASS  $pkg  first compile ${t1}s, second build ${t2}s: served from the cache  ($what)"
  exit 0
fi
echo "FAIL  $pkg  first compile ${t1}s, second build ${t2}s: compiled again, not served  ($what)"
exit 1
