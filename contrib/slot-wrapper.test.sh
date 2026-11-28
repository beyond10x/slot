#!/bin/sh
# Test of contrib/slot-wrapper and its helpers (tree-cd.sh, env-names.sh, refusal-log.sh): node
# route, hold files, the toolchain parity wait, the refusals, --here and --local, the return sync
# switch, the tree of --manifest-path, the forwarded variables and the refusal log. HOME is a
# temporary directory with the wrapper installed as `task node-wrapper` installs it; stubs stand
# in for the real slot, git, rustup and rsync. Nothing is built. Run from the repository root.
set -u
# A run inside a slot job (SLOT_JOB) or with a node named (SLOT_HOST) would skip the wrapper.
unset SLOT_JOB SLOT_HOST SLOT_RETURN_SYNC SLOT_SYNC_IGNORED SLOT_SHIM_PID
src=${1:-contrib}
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
trap 'exit 1' HUP INT TERM
HOME=$T/home
export HOME
lib=$HOME/.local/share/slot
mkdir -p "$lib/shims" "$HOME/.local/bin" "$HOME/.config/slot" "$HOME/.local/state/slot" "$T/bin"
cp "$src/slot-wrapper" "$lib/shims/slot"
cp "$src/tree-cd.sh" "$src/env-names.sh" "$src/refusal-log.sh" "$lib/"
chmod +x "$lib/shims/slot"
W=$lib/shims/slot

# The real slot: `nodes route` answers from $T/route (exit 1 without it); any other call writes
# its arguments, directory and the variables the wrapper sets to $T/ran and exits with $T/rc.
cat > "$HOME/.local/bin/slot" <<'EOF'
#!/bin/sh
if [ "$1 $2" = "nodes route" ]; then
  [ -f "$T/route" ] || { echo "no node for $3"; exit 1; }
  cat "$T/route"
  exit 0
fi
{
  echo "args=$*"
  echo "pwd=$PWD"
  echo "host=${SLOT_HOST:-}"
  echo "reason=${SLOT_LOCAL_REASON:-}"
  echo "return=${SLOT_RETURN_SYNC:-}"
  echo "ignored=${SLOT_SYNC_IGNORED:-}"
  echo "shimpid=${SLOT_SHIM_PID:-}"
  echo "env=${SLOT_ENV:-}"
  echo "path=$PATH"
} > "$T/ran"
[ "$1" = status ] && echo "status of the pool"
exit "$(cat "$T/rc" 2>/dev/null || echo 0)"
EOF
# git: a checkout is a directory holding .stub-git (its lines are the remote URLs).
cat > "$T/bin/git" <<'EOF'
#!/bin/sh
d=$PWD
if [ "$1" = -C ]; then d=$2; shift 2; fi
top=$d
while [ ! -f "$top/.stub-git" ]; do
  [ "$top" = / ] && exit 128
  top=$(dirname "$top")
done
case $1 in
  rev-parse)
    shift
    for a in "$@"; do
      case $a in
        --show-toplevel) echo "$top" ;;
        --git-dir | --git-common-dir) echo "$top/.git" ;;
      esac
    done
    ;;
  remote) while read -r u; do printf 'origin\t%s (fetch)\n' "$u"; done < "$top/.stub-git" ;;
  *) exit 1 ;;
esac
EOF
printf '#!/bin/sh\necho "stable-x86_64-unknown-linux-gnu (default)"\n' > "$T/bin/rustup"
# rsync: the wrapper must not run one, nor put a wrapper of it on PATH.
printf '#!/bin/sh\necho "rsync $*" >> "%s/rsync-calls"\n' "$T" > "$T/bin/rsync"
chmod +x "$HOME/.local/bin/slot" "$T/bin/git" "$T/bin/rustup" "$T/bin/rsync"
export T
PATH=$T/bin:$PATH
export PATH

repo=$HOME/src/app
other=$HOME/src/lib
mkdir -p "$repo/sub" "$other" "$HOME/.cache/scratch/s" "$HOME/.cache/x"
echo "git@example.invalid:o/app.git" > "$repo/.stub-git"
echo "git@example.invalid:o/lib.git" > "$other/.stub-git"
: > "$other/Cargo.toml"
: > "$HOME/.cache/scratch/probe.sh"
mt=$HOME/.local/state/slot/node/n1/slot
mkdir -p "$mt"
printf 'rust stable-x86_64-unknown-linux-gnu 1.99.0 abc clippy,x86_64-unknown-linux-gnu\ngo go1.24\n' > "$mt/toolchains-local.txt"
cp "$mt/toolchains-local.txt" "$mt/toolchains-node.txt"
end_soon=$(($(date +%s) + 3600))
route() {
  printf 'SLOT_NODE=%s\nSLOT_HOST=%s.example\nSLOT_SSH=ssh\nSLOT_REMOTE=slot\nSLOT_NODE_END=%s\nexport SLOT_NODE SLOT_HOST SLOT_SSH SLOT_REMOTE SLOT_NODE_END\n' "$1" "$1" "$2" > "$T/route"
}
on() { : > "$HOME/.config/slot/remote-node.env"; }
off() { rm -f "$HOME/.config/slot/remote-node.env"; }

fail=0
ok() { echo "ok   $1"; }
bad() {
  echo "FAIL $1"
  fail=1
}
# run <dir> <args>...: runs the wrapper in <dir>, without SLOT_HOST and SLOT_JOB of the caller;
# sets out (stderr and stdout) and rc, and empties $T/ran first.
run() {
  rm -f "$T/ran"
  d=$1
  shift
  out=$(cd "$d" && env -u SLOT_HOST -u SLOT_JOB -u SLOT_RETURN_SYNC -u SLOT_SYNC_IGNORED -u SLOT_SHIM_PID \
    SLOT_PARITY_WAIT_S=1 SLOT_PARITY_POLL_S=1 SLOT_HOLD_POLL_S=1 "$W" "$@" 2>&1)
  rc=$?
}
ran() { grep -qx "$1" "$T/ran" 2>/dev/null; }
said() { case $out in *"$1"*) return 0 ;; *) return 1 ;; esac; }
logged() { grep -q "$1" "$HOME/.local/state/slot/refusals.log" 2>/dev/null; }

# Switched off: the real slot runs unchanged, no route asked.
off
route n1 "$end_soon"
run "$repo" cargo build
{ [ $rc -eq 0 ] && ran "args=cargo build" && ran "host=" && ran "reason="; } &&
  ok "switch off: the job runs here unchanged" || bad "switch off: rc=$rc $out"

# Node route: the job runs with the node's terms, the return sync on, the variables named.
on
export FOO_SETTING=1 SECRET_TOKEN=x
run "$repo/sub" cargo build
unset FOO_SETTING SECRET_TOKEN
{ [ $rc -eq 0 ] && ran "args=cargo build" && ran "host=n1.example" && ran "pwd=$repo/sub"; } &&
  ok "route: the job goes to the node the route names" || bad "route: rc=$rc $out"
ran "return=1" && ran "shimpid=" &&
  ok "return sync: on, and no wrapper mark turns the client's off" || bad "return sync: $(cat "$T/ran")"
ran "ignored=1" && ok "sync: ignored files go to the node" || bad "sync ignored: $(cat "$T/ran")"
{ grep -q '^env=.*FOO_SETTING' "$T/ran" && ! grep -q '^env=.*SECRET_TOKEN' "$T/ran" &&
  ! grep -Eq '^env=(.*,)?(PATH|HOME)(,|$)' "$T/ran"; } &&
  ok "env-names: the job's variables go along, credentials and session ones do not" ||
  bad "env-names: $(grep '^env=' "$T/ran")"
{ [ ! -e "$T/rsync-calls" ] && ! grep -q '^path=.*rsync' "$T/ran"; } &&
  ok "sync: no rsync wrapper on PATH, the client syncs" || bad "sync: rsync wrapper used"
# The caller's own switch: run unsets it, so call directly.
rm -f "$T/ran"
(cd "$repo" && env -u SLOT_HOST -u SLOT_JOB SLOT_RETURN_SYNC=0 "$W" cargo build > /dev/null 2>&1)
ran "return=0" && ok "return sync: a caller's 0 turns it off" || bad "return sync off: $(cat "$T/ran")"
rm -f "$T/ran"
(cd "$repo" && env -u SLOT_HOST -u SLOT_JOB SLOT_SYNC_IGNORED=0 "$W" cargo build > /dev/null 2>&1)
ran "ignored=0" && ok "sync: a caller's 0 keeps every .gitignore" || bad "sync ignored off: $(cat "$T/ran")"

# No route: refused with exit 3, logged.
rm -f "$T/route"
run "$repo" cargo build
{ [ $rc -eq 3 ] && said "no registered build node for repository 'app'" && [ ! -e "$T/ran" ] &&
  logged "node route: cargo build"; } &&
  ok "route: no registered node refuses the job (exit 3)" || bad "no route: rc=$rc $out"
route n1 "$end_soon"

# Refusals of a compile under ~/.cache, of --here for a compile, of a scratch script.
run "$HOME/.cache/x" cargo build
{ [ $rc -eq 3 ] && said "a compile under ~/.cache runs on this host and is not sent to n1" &&
  [ ! -e "$T/ran" ] && logged "cargo build"; } &&
  ok "refusal: a compile under ~/.cache (exit 3)" || bad "cache compile: rc=$rc $out"
run "$repo" --here cargo test
{ [ $rc -eq 3 ] && said "--here is for a step that needs this host's credentials" && [ ! -e "$T/ran" ]; } &&
  ok "refusal: --here for a compile (exit 3)" || bad "here compile: rc=$rc $out"
run "$repo" sh "$HOME/.cache/scratch/probe.sh"
{ [ $rc -eq 3 ] && said "runs a file under ~/.cache/scratch" && [ ! -e "$T/ran" ]; } &&
  ok "refusal: a scratch script on a tree outside it (exit 3)" || bad "scratch: rc=$rc $out"
line=$(tail -n 1 "$HOME/.local/state/slot/refusals.log")
[ "$(printf '%s\n' "$line" | awk -F '\t' '{print NF ":" $2 ":" $4}')" = "5:wrapper:$repo" ] &&
  ok "refusal log: time, kind, session, directory, text" || bad "refusal log: $line"

# --here and --local for a step that compiles nothing; jobs under ~/.cache run here.
run "$repo" --here git push
{ [ $rc -eq 0 ] && ran "args=git push" && ran "reason=here: needs this host" && ran "host="; } &&
  ok "--here: runs in this host's pool without --here" || bad "here: rc=$rc $(cat "$T/ran" 2>/dev/null)"
run "$repo" --local cargo build
{ [ $rc -eq 0 ] && ran "args=--local cargo build" && ran "reason=local requested"; } &&
  ok "--local: runs here" || bad "local: rc=$rc $out"
run "$HOME/.cache/x" sh -c true
{ [ $rc -eq 0 ] && ran "reason=cache rule" && ran "host="; } &&
  ok "cache rule: a job under ~/.cache runs here" || bad "cache rule: rc=$rc $out"
run "$HOME" sh -c true
{ [ $rc -eq 0 ] && ran "reason=never sync: home directory"; } &&
  ok "home: a tree of the home directory runs here" || bad "home: rc=$rc $out"
echo "$other" > "$HOME/.config/slot/remote-deny"
run "$other" cargo build
{ [ $rc -eq 0 ] && ran "reason=never sync: $other"; } &&
  ok "remote-deny: a listed tree runs here" || bad "deny: rc=$rc $out"
echo "origin:*o/app.git" > "$HOME/.config/slot/remote-deny"
run "$repo" cargo build
{ [ $rc -eq 0 ] && ran "reason=never sync: remote git@example.invalid:o/app.git"; } &&
  ok "remote-deny: a listed origin runs here" || bad "deny origin: rc=$rc $out"
rm -f "$HOME/.config/slot/remote-deny"
rm -f "$T/ran"
(cd "$repo" && SLOT_JOB=1 "$W" cargo build > /dev/null 2>&1)
ran "host=" && ran "reason=" && ok "inside a job: runs as asked" || bad "inside a job"

# The tree of --manifest-path: a manifest in another checkout moves the job there.
run "$repo" cargo build --manifest-path "$other/Cargo.toml"
{ [ $rc -eq 0 ] && ran "pwd=$other" && said "slot: building in $other (the tree of --manifest-path)"; } &&
  ok "tree-cd: the job runs from the tree of --manifest-path" || bad "tree-cd: rc=$rc $out"

# Hold files: of this node, of every node; another node's does not hold this one.
echo "maintenance" > "$HOME/.config/slot/node-hold.n2"
run "$repo" cargo build
[ $rc -eq 0 ] && ok "hold: another node's hold file does not hold the job" || bad "hold n2: rc=$rc $out"
rm -f "$HOME/.config/slot/node-hold.n2"
echo "image update" > "$HOME/.config/slot/node-hold.n1"
past=$(($(date +%s) - 60))
route n1 "$past"
run "$repo" cargo build
{ [ $rc -eq 3 ] && said "slot: waiting for n1: image update" && said "build node n1 ended" &&
  [ ! -e "$T/ran" ]; } &&
  ok "hold: a held node past its end refuses the job (exit 3)" || bad "hold end: rc=$rc $out"
said "build node n1 ended $(date -d @"$past" '+%H:%M %Z')," &&
  ok "hold: the end is named in this machine's time zone" || bad "hold end zone: $out"
route n1 "$end_soon"
(cd "$repo" && env -u SLOT_HOST -u SLOT_JOB SLOT_HOLD_POLL_S=1 "$W" cargo build > "$T/held.out" 2>&1; echo $? > "$T/held.rc") &
i=0
while [ $i -lt 50 ] && ! grep -q "waiting for n1" "$T/held.out" 2>/dev/null; do sleep 0.1; i=$((i + 1)); done
st=$(cd "$repo" && env -u SLOT_HOST "$W" status 2>&1)
case $st in *"waiting for n1 since "*": image update"*) ok "hold: slot status lists the waiting job" ;; *) bad "hold status: $st" ;; esac
[ ! -e "$T/held.rc" ] && ok "hold: the job waits while the node is held" || bad "hold: ended early"
rm -f "$HOME/.config/slot/node-hold.n1"
wait
{ [ "$(cat "$T/held.rc")" = 0 ] && ran "host=n1.example"; } &&
  ok "hold: the job runs on the node once the hold goes" || bad "hold release: $(cat "$T/held.out")"
: > "$HOME/.config/slot/node-hold"
route n1 "$(($(date +%s) - 60))"
run "$repo" cargo build
[ $rc -eq 3 ] && ok "hold: node-hold holds every node" || bad "hold all: rc=$rc $out"
rm -f "$HOME/.config/slot/node-hold"
route n1 "$end_soon"

# Toolchain parity: a node without this rustc waits, then exit 3; equal manifests run.
: > "$repo/Cargo.toml"
printf 'rust stable-x86_64-unknown-linux-gnu 1.98.0 def clippy,x86_64-unknown-linux-gnu\ngo go1.24\n' > "$mt/toolchains-node.txt"
run "$repo" cargo build
{ [ $rc -eq 3 ] && said "waiting for n1: rust toolchain stable-x86_64-unknown-linux-gnu is not the same on n1 yet (n1 lacks: rustc" &&
  said "gave up after 1 s" && [ ! -e "$T/ran" ] && logged "parity"; } &&
  ok "parity: another rustc on the node waits at most the bound, then exit 3" || bad "parity: rc=$rc $out"
cp "$mt/toolchains-local.txt" "$mt/toolchains-node.txt"
run "$repo" cargo build
[ $rc -eq 0 ] && ok "parity: equal toolchains run" || bad "parity equal: rc=$rc $out"
rm -f "$repo/Cargo.toml"
mv "$mt" "$T/mt"
run "$repo" cargo build
{ [ $rc -eq 3 ] && said "no toolchain manifest of n1 yet"; } &&
  ok "parity: a node without a manifest gets no job" || bad "no manifest: rc=$rc $out"
# No default node: remote-node.env names none, so the manifest of ~/.local/state/slot is no one's.
cp "$T/mt/toolchains-node.txt" "$HOME/.local/state/slot/toolchains-node.txt"
run "$repo" cargo build
[ $rc -eq 3 ] && ok "no default node: a node without its own manifest directory gets no job" || bad "default node: rc=$rc $out"
echo "SLOT_HOST=n1" > "$HOME/.config/slot/remote-node.env"
run "$repo" cargo build
[ $rc -eq 0 ] && ok "the node remote-node.env names reads ~/.local/state/slot" || bad "legacy manifest: rc=$rc $out"
mv "$T/mt" "$mt"
on

# A job that ends with exit 3 after its node's lease ended says so.
route n1 "$(($(date +%s) - 1))"
echo 3 > "$T/rc"
mkdir -p "$mt"
run "$repo" sh -c true
rm -f "$T/rc"
{ [ $rc -eq 3 ] && said "build node n1 ended"; } &&
  ok "lease: a job that ends with exit 3 after its node's end says so" || bad "lease end: rc=$rc $out"
route n1 "$end_soon"

# Help names the node.
run "$repo" --help
said "This tree's node is n1." && ok "help: names this tree's node" || bad "help: $out"
exit $fail
