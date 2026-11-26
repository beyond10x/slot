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
#   mode both     takes tree A only and makes three worktrees of its repository beside it, from
#                 its HEAD, each on a new branch, each with one test that prints the name of its
#                 branch. Every build is a generic job that binds its own <worktree>/target at
#                 ONE absolute path inside a mount namespace of that job alone and names that
#                 path in CARGO_TARGET_DIR, the same value in every worktree; no directory is
#                 shared by two worktrees. Worktree 1 builds alone (the first compile); worktrees
#                 2 and 3 then build and run the test at the same time. Passes when each of the
#                 two printed the value of its own branch and no other worktree's, and neither
#                 build log has a "Blocking waiting for file lock" line (a wait on cargo's package
#                 cache, which any two cargo jobs share, is printed and not counted). For each of
#                 the two it prints the time of the package, how many library units took at most
#                 a third of their first compile, and the Rust hits the cache server counted
#                 meanwhile (all jobs of the machine). A hit costs 0.12 to 0.25 s with
#                 SCCACHE_CLIENT_SIDE=1, so a unit that compiles in under half a second is a hit
#                 for the server and not "a third" here. Whether the package was served is
#                 printed and is not part of the exit status.
#                 The worktrees, their branches, their mirrors on a build server and the logs are
#                 removed on every exit; the removal on a server is a job too.
#     PROBE_VIEW       the one path (default /mnt/slot-probe-target). Its parent must exist; the
#                      job puts a tmpfs over the parent in its own view.
#     PROBE_VIEW_TOOL  bwrap (keeps the user) or unshare (the build is root of a new user
#                      namespace); default: bwrap when the job finds it. No root, nothing changed
#                      on the machine.
#     PROBE_VIEW_ENV   assignments for the build in the view, default "SCCACHE_CLIENT_SIDE=1":
#                      an sccache server outside the view runs rustc itself and does not see the
#                      path (`error writing dependencies to …: No such file or directory`); with
#                      the variable (sccache 0.18.0) the client compiles and the server only
#                      stores. Set it empty to see the failure.
#     PROBE_MEMBER     the package directory, relative to the tree, that gets the test (default .)
#     PROBE_NO_RUN     set: build the test binaries without running them (a target without a
#                      runner); the check then reads which value each worktree's own binary holds
#     PROBE_SHARE_TARGET  set: the negative control. All three views bind ONE directory, the
#                      case the story rules out; the mode must then exit 1.
#     PROBE_WORKTREE_DIR  where the worktrees go (default: beside the tree)
#     PROBE_KEEP       set: keep the logs and print their directory
# PROBE_CARGO_ARGS in the environment: more cargo arguments for both builds, for example
#   "--target wasm32-unknown-unknown"
#
# Every run compiles with a `--cfg` name of its own (RUSTFLAGS), so the first build cannot be in the
# cache: its time for the package is the time of a real compile under the load of this minute. The
# second build is "served" when its time for the same package is at most a third of that.
# exit 0: served.  exit 1: compiled again.  exit 2: the probe itself did not work.
# Mode both: exit 0 and 1 are its own two checks, as above.
#
# Runs without `slot` in front: each `cargo` goes to the pool by itself, each read is `slot sh -c`.
# Each job is a build of one small package and ends within the pool's 10 minutes.
set -u
[ $# -ge 3 ] || { echo "usage: cache-key-probe.sh <mode> <package spec> <tree A> [<tree B>]"; exit 2; }
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

if [ "$mode" = both ]; then
  view=${PROBE_VIEW:-/mnt/slot-probe-target}
  tool=${PROBE_VIEW_TOOL:-auto}
  venv=${PROBE_VIEW_ENV-SCCACHE_CLIENT_SIDE=1}
  member=${PROBE_MEMBER:-.}
  norun=
  [ -z "${PROBE_NO_RUN:-}" ] || norun=--no-run
  share=-
  tdir=target
  made=
  p2=
  p3=
  # shellcheck disable=SC2329  # called by the trap
  cleanup() {
    rc=$?
    trap - EXIT
    [ -z "$p3" ] || kill "$p2" "$p3" 2> /dev/null
    if [ -n "$made" ]; then
      # One job removes the mirrors of the worktrees on a build server, with their target
      # directories; without a server it removes the worktrees here.
      (cd "$top" && slot sh -c 'for d; do case $d in */slot-probe-*) rm -rf "$d" ;; esac; done' sh \
        "$wdir/slot-probe-$key-1" "$wdir/slot-probe-$key-2" "$wdir/slot-probe-$key-3" "$wdir/slot-probe-$key-shared" > /dev/null 2>&1)
      for n in 1 2 3; do
        git -C "$top" worktree remove --force "$wdir/slot-probe-$key-$n" > /dev/null 2>&1
        rm -rf "$wdir/slot-probe-$key-$n"
      done
      rm -rf "$wdir/slot-probe-$key-shared"
      git -C "$top" worktree prune > /dev/null 2>&1
      for n in 1 2 3; do git -C "$top" branch -q -D "slot-probe-$key-$n" > /dev/null 2>&1; done
    fi
    if [ -n "${PROBE_KEEP:-}" ]; then echo "probe: logs in $out"; else rm -rf "$out"; fi
    exit "$rc"
  }
  trap cleanup EXIT
  trap 'exit 130' INT TERM HUP
  case $view in /*/*) ;; *) echo "probe: PROBE_VIEW is an absolute path below a directory that exists, not $view"; exit 2 ;; esac
  case $tool in auto | bwrap | unshare) ;; *) echo "probe: PROBE_VIEW_TOOL is bwrap or unshare, not $tool"; exit 2 ;; esac
  [ $# -eq 3 ] || { echo "probe: mode both takes one tree and makes its own worktrees"; exit 2; }
  top=$(cd "$a" 2> /dev/null && git rev-parse --show-toplevel 2> /dev/null) || { echo "probe: $a is not in a git checkout"; exit 2; }
  [ -f "$top/$member/Cargo.toml" ] || { echo "probe: no Cargo.toml in $top/$member (PROBE_MEMBER names the package that gets the test)"; exit 2; }
  wdir=${PROBE_WORKTREE_DIR:-$(dirname "$top")}
  up=.
  [ "$member" = . ] || up=$(printf '%s\n' "$member" | sed 's#[^/][^/]*#..#g')
  if [ -n "${PROBE_SHARE_TARGET:-}" ]; then
    share=$wdir/slot-probe-$key-shared/target
    tdir=$share
  fi
  made=1
  for n in 1 2 3; do
    w=$wdir/slot-probe-$key-$n
    git -C "$top" worktree add -q -b "slot-probe-$key-$n" "$w" HEAD > "$out/worktree$n.log" 2>&1 || { echo "probe: git worktree add failed: $(cat "$out/worktree$n.log")"; exit 2; }
    mkdir -p "$w/$member/tests" || exit 2
    printf '#[test]\nfn slot_probe_branch() {\n    println!("slot-probe-branch=%s");\n}\n' "slot-probe-$key-$n" > "$w/$member/tests/slot_probe_branch.rs"
  done
  # What a job runs: $1 the tree's root as seen from the working directory, $2 the one path,
  # $3 bwrap, unshare or auto, $4 - or the one directory of the negative control, then the command.
  viewjob=$(
    cat << 'EOF'
real=$(cd "$1" && pwd)/target || exit 97
[ "$4" = - ] || real=$4
view=$2
tool=$3
shift 4
mkdir -p "$real" || exit 97
hits() { sccache --show-stats 2> /dev/null | awk '/^Cache hits \(Rust\)/ { print "slot-probe-rust-hits " $4 }'; }
if [ "$tool" = auto ]; then
  tool=unshare
  command -v bwrap > /dev/null 2>&1 && tool=bwrap
fi
echo "slot-probe-view $tool $view"
hits
case $tool in
  bwrap) bwrap --dev-bind / / --tmpfs "${view%/*}" --bind "$real" "$view" -- "$@" ;;
  *) unshare --user --map-root-user --mount sh -c 'mount -t tmpfs none "${2%/*}" && mkdir "$2" && mount --bind "$1" "$2" || exit 97; shift 2; exec "$@"' sh "$real" "$view" "$@" ;;
esac
rc=$?
hits
exit "$rc"
EOF
  )
  # view_build <n>: the build of worktree n in its view; its log is $out/build<n>.log
  view_build() {
    cd "$wdir/slot-probe-$key-$1/$member" || return 2
    # shellcheck disable=SC2086
    exec slot sh -c "$viewjob" sh "$up" "$view" "$tool" "$share" env RUSTFLAGS="--cfg $key" CARGO_TARGET_DIR="$view" $venv \
      cargo test -v --locked --timings $more --test slot_probe_branch $norun -- --nocapture > "$out/build$1.log" 2>&1
  }
  # view_time <n>: reads the timing report from worktree n's own target directory and prints the
  # time of the measured package
  view_time() {
    (cd "$wdir/slot-probe-$key-$1" && slot sh -c 'cat "$(ls -t "$1"/cargo-timings/cargo-timing-*.html | sed -n "$2p")"' sh "$tdir" "${2:-1}" > "$out/timing$1.html" 2> "$out/timing$1.err")
    units "$out/timing$1.html" > "$out/units$1.tsv"
    awk -F'\t' -v n="$name" -v v="$ver" '$3=="" { split($2, p, " "); if (p[1]==n && (v=="" || p[2]==v)) print $1 }' "$out/units$1.tsv" | sort -rn | head -1
  }
  failed() {
    echo "probe: build $1 failed:"
    grep -E '^(error|bwrap|unshare|mount|mkdir|slot)|process didn.t exit' "$out/build$1.log" | cut -c1-300 | head -6
  }
  (view_build 1) || { failed 1; exit 2; }
  (view_build 2) &
  p2=$!
  (view_build 3) &
  p3=$!
  wait "$p2"
  r2=$?
  wait "$p3"
  r3=$?
  [ "$r2" = 0 ] || { failed 2; exit 2; }
  [ "$r3" = 0 ] || { failed 3; exit 2; }
  grep -q -- "--cfg $key" "$out/build1.log" || { echo "probe: build 1 did not get the --cfg of this run"; exit 2; }
  # In the one directory of the negative control the first build's report is the oldest of three.
  first=1
  [ "$share" = - ] || first=3
  t1=$(view_time 1 "$first")
  [ -n "$t1" ] || { echo "probe: no time for $pkg in the timing report of the first build"; exit 2; }
  echo "first  worktree 1 alone: $pkg ${t1}s, a compile  ($(sed -n 's/^slot-probe-view //p' "$out/build1.log" | head -1), ${venv:-no variable added})"
  ok=0
  for n in 2 3; do
    other=$((5 - n))
    if [ "$share" = - ]; then
      t=$(view_time "$n")
    else
      t=
      : > "$out/units$n.tsv"
    fi
    if [ -z "$t" ]; then
      t="no time, "
      verdict="not built: cargo found it fresh in the target directory it was given"
    elif awk -v x="$t1" -v y="$t" 'BEGIN { exit !(y * 3 <= x) }'; then
      t="${t}s, "
      verdict="served from the cache"
    else
      t="${t}s, "
      verdict="compiled again"
    fi
    served=$(awk -F'\t' 'NR==FNR { if ($3=="") a[$2]=$1; next } $3=="" && ($2 in a) { n++; if ($1 * 3 <= a[$2]) s++ } END { printf "%d of %d", s, n }' "$out/units1.tsv" "$out/units$n.tsv")
    hits=$(sed -n 's/^slot-probe-rust-hits //p' "$out/build$n.log" | awk 'NR==1 { x=$1 } { y=$1 } END { if (NR > 1) print y - x; else print "not read" }')
    lock=$(grep 'Blocking waiting for file lock' "$out/build$n.log" | grep -c -v 'package cache')
    pkgcache=$(grep 'Blocking waiting for file lock' "$out/build$n.log" | grep -c 'package cache')
    if [ -n "$norun" ]; then
      (cd "$wdir/slot-probe-$key-$n" && slot sh -c 'd=$1; shift; for v; do find "$d" -type f -name "slot_probe_branch-*" ! -name "*.d" -exec grep -lF -- "$v" {} + | wc -l; done' sh \
        "$tdir" "slot-probe-branch=slot-probe-$key-$n" "slot-probe-branch=slot-probe-$key-$other" > "$out/own$n.txt" 2> "$out/own$n.err")
      own=$(sed -n 1p "$out/own$n.txt" | tr -d ' ')
      foreign=$(sed -n 2p "$out/own$n.txt" | tr -d ' ')
      [ -n "$own" ] && [ -n "$foreign" ] || { echo "probe: could not read the test binaries of worktree $n"; exit 2; }
      how="test binary built, not run: $own file(s) in its target directory hold its own branch's value, $foreign the other worktree's"
    else
      own=$(grep -c "^slot-probe-branch=slot-probe-$key-$n\$" "$out/build$n.log")
      foreign=$(grep '^slot-probe-branch=' "$out/build$n.log" | grep -c -v -- "-$n\$")
      how="its test printed its own branch's value $own time(s), another worktree's $foreign time(s)"
      [ "$foreign" = 0 ] || how="$how (worktree $(grep '^slot-probe-branch=' "$out/build$n.log" | grep -v -- "-$n\$" | sed 's/.*-//' | sort -u | tr '\n' ' ' | sed 's/ $//'))"
    fi
    [ "$own" -ge 1 ] && [ "$foreign" = 0 ] && [ "$lock" = 0 ] || ok=1
    note=
    [ "$pkgcache" = 0 ] || note=" (and $pkgcache on cargo's package cache, not counted)"
    echo "both   worktree $n: $pkg $t$verdict; $served library units took at most a third of their first compile; Rust hits the cache server counted meanwhile, all jobs: $hits; $how; lines waiting for a file lock: $lock$note"
  done
  what="worktrees 2 and 3 at the same time, each with its own target directory bound at $view"
  [ "$share" = - ] || what="NEGATIVE CONTROL: worktrees 2 and 3 at the same time, ONE directory bound at $view for all three"
  did=ran
  verb=run
  [ -z "$norun" ] || { did=built; verb=build; }
  if [ "$ok" = 0 ]; then
    echo "PASS  each worktree $did the test binary of its own branch and neither waited for a file lock  ($what)"
    exit 0
  fi
  echo "FAIL  a worktree did not $verb the test binary of its own branch, or waited for a file lock  ($what)"
  exit 1
fi

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
  *) echo "probe: mode is tree, var, flag, runner, jobkind, profile or both"; exit 2 ;;
esac
case $mode in var | flag | runner | jobkind | profile) (cd "$a" && slot sh -c "rm -rf target/$key-1 target/$key-2" > /dev/null 2>&1) ;; esac
[ -n "$t1" ] && [ -n "$t2" ] || { echo "probe: no time for $pkg in the timing report, see $out"; exit 2; }
if awk -v x="$t1" -v y="$t2" 'BEGIN { exit !(y * 3 <= x) }'; then
  echo "PASS  $pkg  first compile ${t1}s, second build ${t2}s: served from the cache  ($what)"
  exit 0
fi
echo "FAIL  $pkg  first compile ${t1}s, second build ${t2}s: compiled again, not served  ($what)"
exit 1
