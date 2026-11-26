#!/bin/sh
# Tests of the node provisioning (provision.sh, conf.sh). POSIX sh, no network, no root: every
# path lives under a temporary NODE_ROOT, and with NODE_TEST=1 the system queries (mounts,
# packages, users, units, attributes, CPUs) read and the system actions write files under
# NODE_ROOT/.state instead of touching the machine.
set -u
here=$(cd "$(dirname "$0")" && pwd)
P=$here/provision.sh
base=${TMPDIR:-/tmp}
W=$(mktemp -d "$base/slot-node-test.XXXXXX")
trap 'rm -rf "$W"' EXIT
# The tests read no file of the user that runs them: toolchains.sh looks in $HOME/.local/bin.
HOME=$W/home
export HOME
mkdir -p "$HOME"
fails=0
ok() { echo "ok - $1"; }
bad() {
  echo "not ok - $1"
  fails=$((fails + 1))
}

me=$(id -un)
uid=$(id -u)
conf() {
  cat >"$W/node.conf" <<EOF
BUILD_USER=$me
BUILD_UID=$uid
BUILD_HOME=/var/lib/builder
BUILD_KEY=ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBuildKeyForTheTestOnly build
SLOT_VERSION=0.36.0
MIRROR_DIRS=.local/state/worktree .cache work
GO_PRIVATE=git.example.org
NODE_NAME=node-1
RUST_TOOLCHAINS=1.99.1 stable
EOF
}
run() { NODE_TEST=1 NODE_ROOT=$1 sh "$P" "$2" "$W/node.conf" >"$W/out" 2>"$W/err"; }

# 1. The config is refused when a required key is missing, a key is unknown or a value is malformed.
conf
grep -v '^BUILD_KEY=' "$W/node.conf" >"$W/c2" && mv "$W/c2" "$W/node.conf"
mkdir -p "$W/r0"
run "$W/r0" check
if [ $? -eq 2 ] && grep -q 'required key BUILD_KEY' "$W/err"; then ok "missing BUILD_KEY is refused"; else bad "missing BUILD_KEY is refused ($(cat "$W/err"))"; fi
conf
echo 'BUILD_SHELL=/bin/zsh' >>"$W/node.conf"
run "$W/r0" check
if [ $? -eq 2 ] && grep -q 'unknown key BUILD_SHELL' "$W/err"; then ok "unknown key is refused"; else bad "unknown key is refused ($(cat "$W/err"))"; fi
for gone in HOME_COPIES=.m2 ETC_EXTRA=java21-openjdk SKIP_BINS=ess; do
  conf
  echo "$gone" >>"$W/node.conf"
  run "$W/r0" check
  if [ $? -eq 2 ] && grep -q "unknown key ${gone%%=*}" "$W/err"; then ok "the copy key ${gone%%=*} is gone from the config"; else bad "the copy key ${gone%%=*} is still accepted ($(cat "$W/err"))"; fi
done
conf
printf 'RUST_COMPONENTS=clippy\nRUST_TARGETS=wasm32-unknown-unknown\n' >>"$W/node.conf"
run "$W/r0" check
if grep -q 'unknown key' "$W/err"; then bad "RUST_COMPONENTS and RUST_TARGETS are refused ($(cat "$W/err"))"; else ok "RUST_COMPONENTS and RUST_TARGETS are config keys"; fi
for bad_target in aarch64-unknown-linux-musl musl x86_64-unknown-linux-gnu.tar.gz 'x86_64-unknown-linux-musl x' '../x86_64-unknown-linux-musl'; do
  conf
  echo "SLOT_TARGET=$bad_target" >>"$W/node.conf"
  run "$W/r0" check
  if [ $? -eq 2 ] && grep -q "SLOT_TARGET must be x86_64-unknown-linux-gnu or x86_64-unknown-linux-musl, not '$bad_target'" "$W/err"; then ok "SLOT_TARGET=$bad_target is refused"; else bad "SLOT_TARGET=$bad_target is accepted ($(cat "$W/err"))"; fi
done
for bad_rust in 'RUST_TOOLCHAINS=stable;id' 'RUST_COMPONENTS=clippy*' 'RUST_TARGETS=$(id)' 'RUST_TARGETS=1.99.1:-t' 'RUST_TARGETS=1.99.1:a,-b' 'RUST_TOOLCHAINS=1.99.1 -stable' 'EXTRA_PACKAGES=jq;id' 'EXTRA_PACKAGES=git --nogpgcheck' 'EXTRA_PACKAGES=a $(id)' 'EXTRA_PACKAGES=a*'; do
  conf
  echo "$bad_rust" >>"$W/node.conf"
  run "$W/r0" check
  if [ $? -eq 2 ] && grep -q "${bad_rust%%=*} must be names" "$W/err"; then ok "${bad_rust%%=*} with a shell character is refused"; else bad "${bad_rust%%=*} with a shell character is accepted ($(cat "$W/err"))"; fi
done
for bad_form in 'RUST_TARGETS=1.99:x86_64-unknown-linux-musl' 'RUST_COMPONENTS=nosuch:rust-src' 'RUST_TARGETS=a,b' 'RUST_COMPONENTS=clippy,rustfmt' 'RUST_TARGETS=1.99.1:a:b' 'RUST_TARGETS=1.99.1:' 'RUST_TARGETS=1.99.1:a,,b' 'RUST_TARGETS=1.99.1:,a' 'RUST_COMPONENTS=llvm-tools-preview' 'RUST_COMPONENTS=stable:clippy-preview,rustfmt'; do
  conf
  echo "$bad_form" >>"$W/node.conf"
  run "$W/r0" check
  if [ $? -eq 2 ] && grep -q "${bad_form%%=*} entry '${bad_form#*=}'" "$W/err"; then ok "$bad_form is refused by name"; else bad "$bad_form is accepted or not named ($(cat "$W/err"))"; fi
done
conf
printf 'RUST_TOOLCHAINS=1.99.1  stable\nRUST_TARGETS=:x\n' >>"$W/node.conf"
run "$W/r0" check
if [ $? -eq 2 ] && grep -q "RUST_TARGETS entry ':x' has an empty toolchain" "$W/err"; then ok "an empty toolchain before the colon is refused, also with a double space in RUST_TOOLCHAINS"; else bad "':x' is accepted ($(cat "$W/err"))"; fi
conf
echo 'RUST_TARGETS=1.99.1:a,b stable:c wasm32-unknown-unknown' >>"$W/node.conf"
run "$W/r0" check
if [ $? -ne 2 ]; then ok "RUST_TARGETS: <toolchain>:<a>,<b> beside a bare name is accepted"; else bad "a valid per-toolchain entry is refused ($(cat "$W/err"))"; fi
conf
sed -i 's/^BUILD_UID=.*/BUILD_UID=10x1/' "$W/node.conf"
run "$W/r0" check
if [ $? -eq 2 ] && grep -q 'BUILD_UID must be a whole number' "$W/err"; then ok "malformed BUILD_UID is refused"; else bad "malformed BUILD_UID is refused ($(cat "$W/err"))"; fi
# CPU_LIMIT (busy CPUs from 1) and CPU_PRESSURE (percent over 0, at most 100): a number with at
# most one decimal, or off. cread reads the config with conf.sh alone and prints both values.
cread() { sh -c '. "$1/conf.sh"; conf_read "$2" BUILD_USER && echo "$CPU_LIMIT|$CPU_PRESSURE"' sh "$here" "$W/node.conf" >"$W/out" 2>"$W/err"; }
conf
cread
[ $? -eq 0 ] && [ "$(cat "$W/out")" = '|' ] && ok "CPU_LIMIT and CPU_PRESSURE are config keys, empty until named (provision.sh has the defaults)" || bad "CPU keys: $(cat "$W/out" "$W/err")"
for good_cpu in 'CPU_LIMIT=1' 'CPU_LIMIT=25.6' 'CPU_LIMIT=off' 'CPU_LIMIT=' 'CPU_PRESSURE=100' 'CPU_PRESSURE=0.5' 'CPU_PRESSURE=62.5' 'CPU_PRESSURE=off' 'CPU_PRESSURE='; do
  conf
  echo "$good_cpu" >>"$W/node.conf"
  cread
  st=$?
  case $good_cpu in CPU_LIMIT=*) want_cpu="${good_cpu#*=}|" ;; *) want_cpu="|${good_cpu#*=}" ;; esac
  [ $st -eq 0 ] && [ "$(cat "$W/out")" = "$want_cpu" ] && ok "$good_cpu is accepted" || bad "$good_cpu is refused ($(cat "$W/out" "$W/err"))"
done
for bad_cpu in 'CPU_LIMIT=0' 'CPU_LIMIT=0.9' 'CPU_LIMIT=x' 'CPU_LIMIT=1.25' 'CPU_LIMIT=-3' 'CPU_LIMIT=1e3' 'CPU_LIMIT=.5' 'CPU_LIMIT=12.' 'CPU_LIMIT=1.2.3' 'CPU_LIMIT=8 --floor 1' 'CPU_LIMIT=Off' 'CPU_PRESSURE=0' 'CPU_PRESSURE=0.0' 'CPU_PRESSURE=101' 'CPU_PRESSURE=100.1' 'CPU_PRESSURE=x' 'CPU_PRESSURE=40.55' 'CPU_PRESSURE=40%' 'CPU_PRESSURE=$(id)'; do
  conf
  echo "$bad_cpu" >>"$W/node.conf"
  cread
  [ $? -eq 2 ] && grep -q "${bad_cpu%%=*} must be .*, not '" "$W/err" && ok "$bad_cpu is refused" || bad "$bad_cpu is accepted ($(cat "$W/out" "$W/err"))"
done
conf
printf 'CPU_LIMIT=off\nCPU_PRESSURE=50\n' >>"$W/node.conf"
cread
[ $? -eq 2 ] && grep -q "CPU_PRESSURE must be off or empty when CPU_LIMIT is off, not '50'" "$W/err" && ok "a pressure mark without a CPU limit is refused: the daemon keeps none" || bad "CPU_LIMIT=off with CPU_PRESSURE=50: $(cat "$W/out" "$W/err")"
conf
printf 'CPU_LIMIT=off\nCPU_PRESSURE=off\n' >>"$W/node.conf"
cread
[ $? -eq 0 ] && ok "CPU_LIMIT=off with CPU_PRESSURE=off is accepted" || bad "both off: $(cat "$W/err")"
# DRAIN_MAX (the bound of a drain), RUN_LIMIT (how long a job of the pool runs, or off) and
# RUN_LIMIT_MAX (the most a job names): durations as the slot command line takes them, seconds
# or a number with s, m or h. dread prints the three values; conf_seconds prints a duration's
# seconds.
dread() { sh -c '. "$1/conf.sh"; conf_read "$2" BUILD_USER && echo "$DRAIN_MAX|$RUN_LIMIT|$RUN_LIMIT_MAX"' sh "$here" "$W/node.conf" >"$W/out" 2>"$W/err"; }
conf
dread
[ $? -eq 0 ] && [ "$(cat "$W/out")" = '30m|10m|30m' ] && ok "DRAIN_MAX, RUN_LIMIT and RUN_LIMIT_MAX are config keys with the defaults 30m, 10m and 30m" || bad "duration keys: $(cat "$W/out" "$W/err")"
for good_d in 'DRAIN_MAX=1m' 'DRAIN_MAX=60' 'DRAIN_MAX=60s' 'DRAIN_MAX=45m' 'DRAIN_MAX=24h' 'DRAIN_MAX=86400' 'RUN_LIMIT=1' 'RUN_LIMIT=90s' 'RUN_LIMIT=30m' 'RUN_LIMIT=off' 'RUN_LIMIT_MAX=10m' 'RUN_LIMIT_MAX=600' 'RUN_LIMIT_MAX=2h'; do
  conf
  echo "$good_d" >>"$W/node.conf"
  dread
  [ $? -eq 0 ] && grep -q "${good_d#*=}" "$W/out" && ok "$good_d is accepted" || bad "$good_d is refused ($(cat "$W/out" "$W/err"))"
done
for bad_d in 'DRAIN_MAX=59' 'DRAIN_MAX=59s' 'DRAIN_MAX=0m' 'DRAIN_MAX=25h' 'DRAIN_MAX=86401' 'DRAIN_MAX=off' 'DRAIN_MAX=30 m' 'DRAIN_MAX=1.5h' 'DRAIN_MAX=30M' 'DRAIN_MAX=m' 'DRAIN_MAX=1h30m' 'DRAIN_MAX=-30m' 'DRAIN_MAX=9999999999' 'RUN_LIMIT=0' 'RUN_LIMIT=0m' 'RUN_LIMIT=x' 'RUN_LIMIT=10 --max 9h' 'RUN_LIMIT=1d' 'RUN_LIMIT=Off' 'RUN_LIMIT=9999999999' 'RUN_LIMIT_MAX=0' 'RUN_LIMIT_MAX=off' 'RUN_LIMIT_MAX=1.5h' 'RUN_LIMIT_MAX=h'; do
  conf
  echo "$bad_d" >>"$W/node.conf"
  dread
  [ $? -eq 2 ] && grep -q "${bad_d%%=*} must be a duration .*, not '" "$W/err" && ok "$bad_d is refused" || bad "$bad_d is accepted ($(cat "$W/out" "$W/err"))"
done
conf
printf 'RUN_LIMIT=31m\n' >>"$W/node.conf"
dread
[ $? -eq 2 ] && grep -q "RUN_LIMIT '31m' is longer than RUN_LIMIT_MAX '30m', the most a job may name" "$W/err" && ok "a run limit over the most a job may name is refused, as the daemon refuses it" || bad "RUN_LIMIT=31m: $(cat "$W/out" "$W/err")"
conf
printf 'RUN_LIMIT=1800s\nRUN_LIMIT_MAX=30m\n' >>"$W/node.conf"
dread
[ $? -eq 0 ] && ok "a run limit equal to the most a job may name, in another form, is accepted" || bad "RUN_LIMIT=1800s with 30m: $(cat "$W/err")"
conf
printf 'RUN_LIMIT=off\nRUN_LIMIT_MAX=5m\n' >>"$W/node.conf"
dread
[ $? -eq 0 ] && ok "RUN_LIMIT=off is accepted under any RUN_LIMIT_MAX" || bad "RUN_LIMIT=off: $(cat "$W/err")"
secs=$(sh -c '. "$1/conf.sh"; for d in 600 600s 10m 1h 90 24h 1s; do conf_seconds "$d"; done' sh "$here" | tr '\n' ' ')
[ "$secs" = '600 600 600 3600 90 86400 1 ' ] && ok "conf_seconds: 600, 600s and 10m are 600 s, 1h is 3600 s" || bad "conf_seconds: $secs"
nots=$(sh -c '. "$1/conf.sh"; for d in "" 0 0m x m 10x 1.5m -5 10M " 10m" 1h30m 1d 1234567890; do conf_seconds "$d" && echo "<- $d"; done' sh "$here" | tr '\n' ' ')
[ -z "$nots" ] && ok "conf_seconds: nothing for what is no duration of at least 1 s (empty, 0, 0m, x, m, 10x, 1.5m, -5, 10M, a space, 1h30m, 1d, 10 digits)" || bad "conf_seconds answers: $nots"

# 2. A fresh machine drifts in every part; apply brings it to 0 drift.
conf
R=$W/r1
mkdir -p "$R"
echo 64 >"$R/.state-nproc"
run "$R" check
st=$?
cp "$W/out" "$W/first-check.out"
n=$(grep -c '^drift: ' "$W/out")
if [ $st -eq 1 ] && [ "$n" -ge 15 ]; then ok "fresh machine: check exits 1 with $n drift lines"; else bad "fresh machine: check exit $st, $n drift lines"; fi
for item in storage binds packages user unit:slot-node.service unit:slot-storage.service keys sshd sudoers slot:/usr/local/bin/slot rustup rust:1.99.1 rust:stable; do
  grep -q "^drift: $item" "$W/out" || bad "fresh machine: no drift line for $item"
done
grep -q '^drift: packages: .*zstd' "$W/out" && grep -q '^drift: packages: .*python3' "$W/out" && grep -q '^drift: packages: .*glibc-static' "$W/out" && grep -q '^drift: packages: .*bubblewrap' "$W/out" && grep -q '^drift: packages: .*socat' "$W/out" && grep -q '^drift: packages: .*libubsan' "$W/out" && ok "check names zstd, python3, glibc-static, bubblewrap, socat and libubsan as packages of a bare node" || bad "base packages: $(grep '^drift: packages' "$W/out" | cut -c1-200)"
grep -qx 'drift: nodebin: /srv/slot/nodebin/glab missing' "$W/out" && grep -qx 'drift: nodebin: /srv/slot/nodebin/gh missing' "$W/out" && ok "check names the glab and gh scripts a bare node lacks" || bad "glab, gh: $(grep nodebin "$W/out")"
grep -qx 'drift: core: /etc/sysctl.d/60-slot-core.conf missing' "$W/out" && grep -qx "drift: core: kernel.core_pattern is '|/usr/lib/systemd/systemd-coredump %P %u %g %s %t %c %h', a pipe: an aborted process waits for the handler" "$W/out" && ok "check names the missing sysctl file and the core pipe of a bare node" || bad "core drift: $(grep '^drift: core' "$W/out")"
grep -qx "drift: cpu-limit: the daemon of the pool gives no answer about its CPU limit (GET http://127.0.0.1:7411/api/cpu-limit as $me, 5 s at most), want '51.2' and '20'" "$W/out" && ok "check on a bare node, where no daemon answers, names the CPU limit it wants: 80 % of its 64 CPUs, and 20 % CPU pressure" || bad "cpu-limit drift: $(grep '^drift: cpu-limit' "$W/out")"
: >"$R/.calls"
run "$R" apply
st=$?
cp "$W/out" "$W/first-apply.out"
[ $st -eq 0 ] && ok "apply exits 0" || bad "apply exit $st: $(tail -3 "$W/err")"
# A naked node needs nothing copied: one apply starts both services and leaves 0 drift.
grep -q '^pending: ' "$W/err" && bad "apply on a naked machine leaves a pending step: $(grep '^pending: ' "$W/err")" || ok "apply on a naked machine leaves nothing pending"
for u in slot-node.service sccache-node.service; do
  [ -f "$R/.state/active/$u" ] && ok "one apply starts $u" || bad "$u not started by the first apply"
done
run "$R" check
st=$?
if [ $st -eq 0 ] && grep -q '^0 drift$' "$W/out"; then ok "after one apply: 0 drift"; else bad "after one apply: exit $st, $(cat "$W/out")"; fi
# Core dumps: the pattern is the file name `core` (no pipe to a handler), loaded from a sysctl file
# that sorts after systemd's 50-coredump.conf, and a job's core limit is 0: the soft one, so a
# job that wants a core file raises its own (the hard limit stays unlimited).
[ "$(cat "$R/etc/sysctl.d/60-slot-core.conf" 2>&1)" = 'kernel.core_pattern=core' ] && grep -qx 'sysctl -q -p /etc/sysctl.d/60-slot-core.conf' "$R/.calls" && grep -qx "changed: core: kernel.core_pattern is 'core' (sysctl -p /etc/sysctl.d/60-slot-core.conf)" "$W/first-apply.out" && ok "apply writes the sysctl file with the one line and loads it" || bad "core: $(cat "$R/etc/sysctl.d/60-slot-core.conf" 2>&1) $(grep sysctl "$R/.calls") $(grep core "$W/first-apply.out")"
for u in slot-node.service sccache-node.service; do
  grep -qx 'LimitCORE=0:infinity' "$R/etc/systemd/system/$u" && [ "$(grep -c '^LimitCORE=' "$R/etc/systemd/system/$u")" -eq 1 ] && ok "$u: LimitCORE=0:infinity, soft 0 and hard unlimited" || bad "$u limits: $(grep '^Limit' "$R/etc/systemd/system/$u")"
done
# The CPU limit of the pool is set once slot-node runs.
grep -qx 'slot cpu-limit 51.2 --pressure 20' "$R/.calls" && grep -qx 'changed: cpu-limit: a queued job is held from 51.2 busy CPUs and 20 % CPU pressure (slot cpu-limit 51.2 --pressure 20)' "$W/first-apply.out" && ok "64 CPUs: the first apply sets slot cpu-limit 51.2 --pressure 20" || bad "cpu-limit: $(grep cpu-limit "$R/.calls" "$W/first-apply.out")"
# The run-time limit of the pool too: 10m, and 30m as the most a job names.
grep -qx "drift: run-limit: the daemon of the pool gives no answer about its run-time limit (GET http://127.0.0.1:7411/api/run-limit as $me, 5 s at most), want '600' s (10m) and '1800' s (30m)" "$W/first-check.out" && ok "check on a bare node names the run-time limit it wants: no daemon answers yet" || bad "run-limit drift: $(grep run-limit "$W/first-check.out")"
grep -qx 'slot run-limit 10m --max 30m' "$R/.calls" && grep -qx 'changed: run-limit: a job that runs longer than 10m is ended (exit 124), and a job names up to 30m (slot run-limit 10m --max 30m)' "$W/first-apply.out" && [ "$(cat "$R/.state/run-limit" "$R/.state/run-limit-max" 2>&1 | tr '\n' ' ')" = '600 1800 ' ] && ok "the first apply sets slot run-limit 10m --max 30m" || bad "run-limit: $(grep run-limit "$R/.calls" "$W/first-apply.out")"
# slot-node.service runs a step as root before each start (the end of a drain, section 6v), with
# a clean environment: the unit's PATH begins with a directory of the build user.
[ "$(grep -n '^ExecStartPre=\|^ExecStart=' "$R/etc/systemd/system/slot-node.service" | cut -d: -f2- | sed 's/^ExecStart=.*/ExecStart=/' | tr '\n' ' ')" = 'ExecStartPre=-+/usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin /bin/sh /usr/local/libexec/slot-node/provision.sh start-pre /etc/slot-node/node.conf ExecStart= ' ] && ! grep -q '^ExecStartPre=' "$R/etc/systemd/system/sccache-node.service" && ok "slot-node.service: one ExecStartPre before ExecStart, as root, failure ignored, with its own PATH; sccache-node.service has none" || bad "ExecStartPre: $(grep '^ExecStart' "$R/etc/systemd/system/slot-node.service" "$R/etc/systemd/system/sccache-node.service")"
# The units use the node's own /usr and /etc.
for u in slot-node.service sccache-node.service; do
  grep -qE 'rootfs|BindReadOnlyPaths|ConditionPathExists|workstation' "$R/etc/systemd/system/$u" && bad "$u still binds or waits for a copy: $(grep -E 'rootfs|BindReadOnlyPaths|ConditionPathExists|workstation' "$R/etc/systemd/system/$u" | head -2)" || ok "$u uses the node's own /usr and /etc"
done
for u in slot-node.service sccache-node.service; do
  grep -q '^Environment=PATH=/srv/slot/nodebin:/var/lib/builder/.cargo/bin:/usr/local/bin:/usr/bin:' "$R/etc/systemd/system/$u" && ok "$u PATH: nodebin, cargo, /usr/local/bin, /usr/bin" || bad "$u PATH: $(grep PATH "$R/etc/systemd/system/$u")"
  grep -q '/usr/local/go/bin' "$R/etc/systemd/system/$u" && bad "$u PATH still has /usr/local/go/bin" || ok "$u PATH: no /usr/local/go/bin (the go link is in /usr/local/bin)"
done
# `ip` and the other tools of /usr/sbin: jobs find them as they do on the workstation.
grep -q '^Environment=PATH=[^ ]*:/usr/bin:/usr/sbin:' "$R/etc/systemd/system/slot-node.service" && ok "slot-node.service PATH has /usr/sbin" || bad "slot-node.service PATH: $(grep PATH "$R/etc/systemd/system/slot-node.service")"
grep -qx 'ExecStart=/var/lib/builder/.cargo/bin/sccache' "$R/etc/systemd/system/sccache-node.service" && ok "sccache-node runs the pinned sccache in the store's cargo bin" || bad "sccache ExecStart: $(grep ExecStart "$R/etc/systemd/system/sccache-node.service")"
[ -e "$R/srv/slot/rootfs" ] && bad "the rootfs directory is made" || ok "no rootfs directory"
[ -e "$R/etc/slot-node/copies" ] && bad "the copy list file is written" || ok "no copy list file"
grep -q 'slot-toolchains.sh "" /srv/slot/rustup ' "$R/etc/systemd/system/slot-toolchains-manifest.service" && ok "the manifest runs with prefix \"\" and the store's rustup home" || bad "manifest unit: $(grep ExecStart "$R/etc/systemd/system/slot-toolchains-manifest.service")"
# The compile cache refuses every incremental rustc call; node jobs build without it.
sed -n '/^\[build\]/,/^\[/p' "$R/srv/slot/cargo/config.toml" | grep -qx 'incremental = false' && ok "node cargo config: incremental off" || bad "node cargo config: $(cat "$R/srv/slot/cargo/config.toml")"
# Rust: rustup at the pinned release for the build user, then each toolchain with the default
# components. Nothing is downloaded under NODE_TEST=1; the commands are recorded.
grep -qx 'fetch https://static.rust-lang.org/rustup/archive/1.29.3/x86_64-unknown-linux-gnu/rustup-init' "$R/.calls" && ok "rustup-init 1.29.3 is fetched from the static host" || bad "rustup-init fetch: $(cat "$R/.calls")"
grep -qx 'rustup-init --no-modify-path --default-toolchain none --profile minimal -y' "$R/.calls" && ok "rustup-init runs without a default toolchain or path edits" || bad "rustup-init call: $(cat "$R/.calls")"
for t in 1.99.1 stable; do
  grep -qx "rustup toolchain install $t --profile minimal -c clippy -c rustfmt" "$R/.calls" && ok "toolchain $t is installed with clippy and rustfmt" || bad "toolchain $t call: $(cat "$R/.calls")"
done
grep -q '^changed: rustup' "$W/first-apply.out" && grep -q '^changed: rust:1.99.1' "$W/first-apply.out" && ok "apply reports rustup and the toolchains as changed" || bad "rust changed lines: $(grep rust "$W/first-apply.out")"
run "$R" apply
grep -q '^changed: ' "$W/out" && bad "a second apply changed: $(grep '^changed: ' "$W/out" | head -3)" || ok "a second apply changes nothing"

# 3. Slot count follows the CPUs found.
grep -q -- '--tokens 64 ' "$R/etc/systemd/system/slot-node.service" && ok "64 CPUs: --tokens 64" || bad "64 CPUs: $(grep ExecStart "$R/etc/systemd/system/slot-node.service")"
echo 32 >"$R/.state-nproc"
run "$R" check
grep -q '^drift: unit:slot-node.service' "$W/out" && ok "32 CPUs: slot-node.service drifts" || bad "32 CPUs: $(cat "$W/out")"
grep -qx "drift: cpu-limit: the pool holds a queued job from '51.2' busy CPUs and '20' % CPU pressure, want '25.6' and '20'" "$W/out" && ok "32 CPUs: the CPU limit of 64 CPUs drifts" || bad "32 CPUs, cpu-limit: $(grep cpu-limit "$W/out")"
run "$R" apply
grep -q -- '--tokens 32 ' "$R/etc/systemd/system/slot-node.service" && ok "32 CPUs: --tokens 32" || bad "32 CPUs: $(grep ExecStart "$R/etc/systemd/system/slot-node.service")"
[ "$(cat "$R/.state/cpu-limit" 2>&1)" = 25.6 ] && [ "$(cat "$R/.state/cpu-pressure" 2>&1)" = 20 ] && ok "32 CPUs: the CPU limit is 25.6 busy CPUs" || bad "32 CPUs, marks: $(cat "$R/.state/cpu-limit" "$R/.state/cpu-pressure" 2>&1)"

# 4. Each kind of change on the machine is found, named, and closed by apply.
echo 'Environment=EXTRA=1' >>"$R/etc/systemd/system/sccache-node.service"
echo "/srv/slot/home/.cache /var/lib/builder/.cache none bind 0 0" >>"$R/etc/fstab"
k=$R/var/lib/builder/.ssh/authorized_keys
rm -f "$R/.state/immutable/$(echo "$k" | tr / _)"
echo 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAnotherKey other' >>"$k"
printf '#!/bin/sh\necho slot 0.35.0\n' >"$R/srv/slot/bin/slot"
rm -f "$R/var/lib/builder/.cargo"
rm -f "$R/usr/bin/cargo"
ln -sf /nowhere "$R/usr/bin/go"
grep -v '^MaxSessions' "$R/etc/ssh/sshd_config" >"$W/s" && cat "$W/s" >"$R/etc/ssh/sshd_config"
sed -i '\# /var/lib/builder/.cache$#d' "$R/.state/mounts"
run "$R" check
st=$?
for item in unit:sccache-node.service fstab keys slot:/srv/slot/bin/slot link:/var/lib/builder/.cargo link:/usr/bin/cargo link:/usr/bin/go sshd binds; do
  grep -q "^drift: $item" "$W/out" && ok "found: $item" || bad "not found: $item ($(cat "$W/out"))"
done
[ $st -eq 1 ] || bad "check with drift exits $st"
run "$R" apply
run "$R" check
st=$?
if [ $st -eq 0 ]; then ok "apply closes every drift"; else bad "left after apply: $(cat "$W/out")"; fi
# The shims of the workstation send the real binary by absolute path (/usr/bin/cargo, /usr/bin/go):
# the node has those names as links to its own binaries.
for l in cargo:/var/lib/builder/.cargo/bin/cargo rustc:/var/lib/builder/.cargo/bin/rustc rustup:/var/lib/builder/.cargo/bin/rustup cargo-clippy:/var/lib/builder/.cargo/bin/cargo-clippy go:/usr/local/bin/go gofmt:/usr/local/bin/gofmt; do
  [ "$(readlink "$R/usr/bin/${l%%:*}")" = "${l#*:}" ] && ok "/usr/bin/${l%%:*} links to ${l#*:}" || bad "/usr/bin/${l%%:*}: '$(readlink "$R/usr/bin/${l%%:*}")'"
done
# A real file at one of those names is never replaced.
rm -f "$R/usr/bin/rustc"
printf 'real\n' >"$R/usr/bin/rustc"
run "$R" apply
grep -q '^failed: link:/usr/bin/rustc: is a real file; not replaced' "$W/err" && [ "$(cat "$R/usr/bin/rustc")" = real ] && ok "a real /usr/bin/rustc is not replaced" || bad "real rustc: $(cat "$W/err")"
rm -f "$R/usr/bin/rustc"
run "$R" apply
grep -c 'AnotherKey' "$k" | grep -q '^0$' && ok "an extra key is removed" || bad "the extra key stayed"
grep -q '/var/lib/builder/.cache none bind' "$R/etc/fstab" && bad "the fstab bind line stayed" || ok "fstab bind lines into the store are removed"

# 4b. Root never writes or mounts through a link the build user planted.
mkdir -p "$W/outside"
rm -rf "$R/var/lib/builder/.config/sccache"
ln -s "$W/outside" "$R/var/lib/builder/.config/sccache"
sed -i '\# /var/lib/builder/work$#d' "$R/.state/mounts"
rm -rf "$R/var/lib/builder/work"
ln -s "$W/outside" "$R/var/lib/builder/work"
run "$R" apply
st=$?
[ $st -eq 1 ] && ok "apply with planted links exits 1" || bad "apply with planted links exit $st"
grep -q '^failed: sccache-config: /var/lib/builder/.config/sccache/config has a symbolic link' "$W/err" && ok "no write through a planted link" || bad "write through link: $(cat "$W/err")"
grep -q '^failed: binds: .*/var/lib/builder/work has a symbolic link' "$W/err" && ok "no bind onto a planted link" || bad "bind onto link: $(cat "$W/err")"
[ -z "$(ls -A "$W/outside")" ] && ok "nothing written outside" || bad "written outside: $(ls -A "$W/outside")"
rm -f "$R/var/lib/builder/.config/sccache" "$R/var/lib/builder/work"
run "$R" apply
run "$R" check
[ $? -eq 0 ] && ok "after the links are gone: 0 drift" || bad "after links: $(cat "$W/out")"

# 5. A setup key is present only while asked for.
conf
echo 'SETUP_KEY=ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAISetupKeyForTheTest setup' >>"$W/node.conf"
NODE_TEST=1 NODE_ROOT=$R sh "$P" apply "$W/node.conf" --open >"$W/out" 2>"$W/err"
grep -q 'SetupKeyForTheTest' "$k" && ok "--open adds the setup key" || bad "--open: $(cat "$k")"
run "$R" check
grep -q '^drift: keys' "$W/out" && ok "an open setup key is drift" || bad "open setup key not reported"
run "$R" apply
grep -q 'SetupKeyForTheTest' "$k" && bad "apply left the setup key" || ok "apply removes the setup key"

# 5b. Every slot path is read back after the install; a failed install is never reported as done.
for p in srv/slot/bin/slot usr/local/bin/slot var/lib/builder/.local/bin/slot; do
  printf '#!/bin/sh\necho slot 0.35.0\n' >"$R/$p"
done
run "$R" apply
st=$?
for p in /srv/slot/bin/slot /usr/local/bin/slot /var/lib/builder/.local/bin/slot; do
  [ "$("$R$p" --version 2>/dev/null)" = "slot 0.36.0" ] && ok "installed and read back: $p" || bad "not installed: $p"
done
[ $st -eq 0 ] && ok "apply that installs slot exits 0" || bad "apply that installs slot exit $st: $(cat "$W/err")"
rm -f "$R/srv/slot/bin/slot"
: >"$R/.state-slot-fetch-fails"
run "$R" apply
st=$?
rm -f "$R/.state-slot-fetch-fails"
[ $st -eq 1 ] && ok "a failed slot download: apply exits 1" || bad "a failed slot download: apply exit $st"
grep -q '^failed: slot: download' "$W/err" && ok "a failed slot download is named" || bad "failed download not named: $(cat "$W/err")"
grep -q '^changed: slot' "$W/out" && bad "a failed install is reported as changed" || ok "a failed install is not reported as changed"
run "$R" apply
run "$R" check
[ $? -eq 0 ] && ok "after the download works: 0 drift" || bad "after the download: $(cat "$W/out")"
# SLOT_TARGET: which archive of the release a node installs. The default is the build against
# glibc (x86_64-unknown-linux-gnu), as before the key; x86_64-unknown-linux-musl is the static
# one. `slot --version` answers the same for both, so check also reads which build a binary is,
# at each path and for the running daemon. Under NODE_TEST=1 the stand-in of the musl build
# carries its target in a comment line.
tlines() { cat "$W/out" "$W/err" | grep 'slot' | tr '\n' ' '; }
rm -f "$R/usr/local/bin/slot"
: >"$R/.calls"
run "$R" apply
grep -qx 'fetch slot 0.36.0' "$R/.calls" && grep -qx 'changed: slot: slot 0.36.0 installed at /usr/local/bin/slot' "$W/out" && ok "without SLOT_TARGET the archive, the call and the line of an install are the ones before the key" || bad "default target: $(grep fetch "$R/.calls") $(tlines)"
echo 'SLOT_TARGET=x86_64-unknown-linux-gnu' >>"$W/node.conf"
run "$R" check
! grep -q '^drift: slot\|^drift: running' "$W/out" && grep -qx '1 drift' "$W/out" && grep -q '^drift: libexec: /etc/slot-node/node.conf' "$W/out" && ok "SLOT_TARGET=x86_64-unknown-linux-gnu is the default by name: no drift of slot (the one line is the node's copy of the config file)" || bad "gnu by name: $(cat "$W/out" "$W/err")"
conf
echo 'SLOT_TARGET=x86_64-unknown-linux-musl' >>"$W/node.conf"
run "$R" check
st=$?
n=0
for p in /srv/slot/bin/slot /usr/local/bin/slot /var/lib/builder/.local/bin/slot; do
  grep -qx "drift: slot:$p: is the build for x86_64-unknown-linux-gnu, config says x86_64-unknown-linux-musl" "$W/out" && n=$((n + 1))
done
[ $st -eq 1 ] && [ $n -eq 3 ] && ok "SLOT_TARGET=x86_64-unknown-linux-musl at the same version: check names each path as the other build" || bad "musl check: exit $st, $n of 3 lines: $(tlines)"
echo 0 >"$R/.state/jobs"
: >"$R/.calls"
run "$R" apply
st=$?
[ $st -eq 0 ] && grep -qx 'fetch slot 0.36.0 x86_64-unknown-linux-musl' "$R/.calls" && grep -qx 'changed: slot: slot 0.36.0 (x86_64-unknown-linux-musl) installed at /srv/slot/bin/slot /usr/local/bin/slot /var/lib/builder/.local/bin/slot' "$W/out" && ok "apply fetches the musl archive and installs it at the three paths" || bad "musl apply: exit $st $(grep fetch "$R/.calls") $(tlines)"
grep -qx 'systemctl restart slot-node.service' "$R/.calls" && [ "$(cat "$R/.state/running-target/slot-node.service" 2>/dev/null)" = x86_64-unknown-linux-musl ] && ok "with no job running the daemon restarts on the other build" || bad "musl restart: $(grep systemctl "$R/.calls" | tr '\n' ' ') running '$(cat "$R/.state/running-target/slot-node.service" 2>&1)'"
[ "$("$R/srv/slot/bin/slot" --version)" = 'slot 0.36.0' ] && ok "the musl build answers the same version" || bad "musl stand-in: $("$R/srv/slot/bin/slot" --version 2>&1)"
run "$R" check
[ $? -eq 0 ] && ok "after the install of the musl build: 0 drift" || bad "musl after apply: $(cat "$W/out")"
# A daemon that still runs the other build (its restart waited for jobs, and the binary at the
# path is already the new one): the running process is read, not the file.
echo x86_64-unknown-linux-gnu >"$R/.state/running-target/slot-node.service"
run "$R" check
st=$?
[ $st -eq 1 ] && grep -qx 'drift: running: slot-node.service: it runs the slot build for x86_64-unknown-linux-gnu, the config says x86_64-unknown-linux-musl' "$W/out" && ok "check names a daemon that still runs the other build" || bad "running target: exit $st $(cat "$W/out")"
: >"$R/.calls"
run "$R" apply
grep -qx 'systemctl restart slot-node.service' "$R/.calls" && ! grep -q '^fetch slot' "$R/.calls" && ok "apply restarts that daemon and downloads nothing" || bad "running target apply: $(cat "$R/.calls" | tr '\n' ' ')"
# Back to the default: the gnu archive again, and nothing of the key is left.
conf
run "$R" check
grep -qx 'drift: slot:/usr/local/bin/slot: is the build for x86_64-unknown-linux-musl, config says x86_64-unknown-linux-gnu' "$W/out" && ok "without the key a musl build is drift" || bad "back to gnu check: $(tlines)"
: >"$R/.calls"
run "$R" apply
grep -qx 'fetch slot 0.36.0' "$R/.calls" && grep -qx 'changed: slot: slot 0.36.0 installed at /srv/slot/bin/slot /usr/local/bin/slot /var/lib/builder/.local/bin/slot' "$W/out" && ok "apply installs the gnu archive again" || bad "back to gnu apply: $(grep fetch "$R/.calls") $(tlines)"
run "$R" check
[ $? -eq 0 ] && ok "back at the default: 0 drift" || bad "back to gnu: $(cat "$W/out")"
# How a real binary is told apart: the build against glibc names a program interpreter, the
# static one has program headers and none. A file readelf cannot read gives no answer.
st_o=$(
  eval "$(sed -n '/^q_slot_target() {$/,/^}$/p' "$P")"
  T=
  as_user() { echo "$*" >>"$W/st-cmd"; cat "$W/st-answer"; }
  printf 'Program Headers:\n  INTERP 0x318\n      [Requesting program interpreter: /lib64/ld-linux-x86-64.so.2]\n  LOAD 0x0\n' >"$W/st-answer"
  q_slot_target /a/slot
  printf 'Program Headers:\n  LOAD 0x0\n  DYNAMIC 0x1\n' >"$W/st-answer"
  q_slot_target /a/slot
  : >"$W/st-answer"
  q_slot_target /a/slot
  as_user() { return 1; }
  q_slot_target /a/slot
  echo end
)
[ "$st_o" = "x86_64-unknown-linux-gnu
x86_64-unknown-linux-musl
end" ] && [ "$(head -n 1 "$W/st-cmd")" = 'env LC_ALL=C readelf -lW /a/slot' ] && ok "a binary with a program interpreter is the gnu build, one with program headers and none the musl build; no answer from readelf is no answer" || bad "q_slot_target: $st_o / $(head -n 1 "$W/st-cmd" 2>&1)"

# 5e. Cargo tools pinned in provision.sh (the release a consumer's CI installs) are installed at
# that release in the store's cargo bin and read back; check names one of another release.
sh "$P" pins >"$W/pins" 2>"$W/err"
grep -qx 'wasm-bindgen 0.2.130' "$W/pins" && grep -qx 'wasm-bindgen-test-runner 0.2.130' "$W/pins" &&
  ok "pins: wasm-bindgen 0.2.130" || bad "pins: $(cat "$W/pins" "$W/err")"
# geckodriver at the release of the forge runner image of the browser test lanes.
grep -qx 'geckodriver 0.37.1' "$W/pins" && ok "pins: geckodriver 0.37.1" || bad "pins: no geckodriver 0.37.1: $(cat "$W/pins")"
# sccache: the node has no /usr from the workstation, and the distribution has no package of it.
grep -qx 'sccache 0.18.0' "$W/pins" && ok "pins: sccache 0.18.0" || bad "pins: no sccache 0.18.0: $(cat "$W/pins")"
# ess at the release the workstation has on PATH (a dispatcher: a repository's exact `requires` runs
# that release from the build user's ~/.cache/ess/toolchains, which ess downloads itself), and
# cargo-about at the release a consumer's CI installs.
grep -qx 'ess 0.99.2' "$W/pins" && ok "pins: ess 0.99.2" || bad "pins: no ess 0.99.2: $(cat "$W/pins")"
grep -qx 'cargo-about 0.9.1' "$W/pins" && ok "pins: cargo-about 0.9.1" || bad "pins: no cargo-about 0.9.1: $(cat "$W/pins")"
while read -r b v; do
  [ "$("$R/srv/slot/cargo/bin/$b" --version 2>/dev/null)" = "$b $v" ] && ok "pinned tool installed: $b $v" || bad "pinned tool $b: '$("$R/srv/slot/cargo/bin/$b" --version 2>&1)'"
done <"$W/pins"
printf '#!/bin/sh\necho wasm-bindgen 0.2.129\n' >"$R/srv/slot/cargo/bin/wasm-bindgen"
run "$R" check
grep -q "^drift: tool:wasm-bindgen: .*'wasm-bindgen 0.2.129', want wasm-bindgen 0.2.130" "$W/out" && ok "check names a wasm-bindgen of another release" || bad "wasm-bindgen 0.2.129 not named: $(grep tool "$W/out")"
printf '#!/bin/sh\necho ess 0.99.1\n' >"$R/srv/slot/cargo/bin/ess"
printf '#!/bin/sh\necho cargo-about 0.9.0\n' >"$R/srv/slot/cargo/bin/cargo-about"
run "$R" check
grep -q "^drift: tool:ess: .*'ess 0.99.1', want ess 0.99.2" "$W/out" && ok "check names an ess of another release" || bad "ess 0.99.1 not named: $(grep tool "$W/out")"
grep -q "^drift: tool:cargo-about: .*'cargo-about 0.9.0', want cargo-about 0.9.1" "$W/out" && ok "check names a cargo-about of another release" || bad "cargo-about 0.9.0 not named: $(grep tool "$W/out")"
# aep is built from its tag's commit on the node (SOURCE_PINS): its pin is a git revision.
grep -qx 'aep 0.80.0' "$W/pins" && ok "pins: aep 0.80.0" || bad "pins: no aep 0.80.0: $(cat "$W/pins")"
printf '#!/bin/sh\necho aep 0.79.0\n' >"$R/srv/slot/cargo/bin/aep"
run "$R" check
grep -q "^drift: tool:aep: .*'aep 0.79.0', want aep 0.80.0 (built from https://github.com/beyond10x/aep at af11af152de1732383a5edee0da723bb72d9acb4)" "$W/out" && ok "check names an aep of another release" || bad "aep 0.79.0 not named: $(grep tool "$W/out")"
: >"$R/.state-tool-fetch-fails"
run "$R" apply
st=$?
rm -f "$R/.state-tool-fetch-fails"
[ $st -eq 1 ] && grep -q '^failed: tool:wasm-bindgen: download or checksum of wasm-bindgen 0.2.130' "$W/err" && ok "a failed tool download: apply exits 1 and names it" || bad "failed tool download: exit $st $(cat "$W/err")"
grep -q '^failed: tool:aep: cargo install of aep 0.80.0 failed' "$W/err" && ok "a failed aep build is named" || bad "aep build failure not named: $(cat "$W/err")"
grep -q '^changed: tool:aep' "$W/out" && bad "a failed aep build is reported as changed" || ok "a failed aep build is not reported as changed"
grep -q '^changed: tool:wasm-bindgen' "$W/out" && bad "a failed tool install is reported as changed" || ok "a failed tool install is not reported as changed"
run "$R" apply
grep -q '^changed: tool:wasm-bindgen: wasm-bindgen 0.2.130 installed: wasm-bindgen$' "$W/out" && ok "apply installs the pinned wasm-bindgen" || bad "apply: $(cat "$W/out" "$W/err" | grep tool)"
run "$R" check
[ $? -eq 0 ] && ok "after the tool install: 0 drift" || bad "after the tool install: $(cat "$W/out")"
# A tool whose release tag is not its version (geckodriver's `v0.37.1`) and whose archive holds
# the binary at its top; its `--version` says more than name and version on the first line.
rm -f "$R/srv/slot/cargo/bin/geckodriver"
: >"$R/.calls"
run "$R" apply
grep -qx 'fetch https://github.com/mozilla/geckodriver/releases/download/v0.37.1/geckodriver-v0.37.1-linux64.tar.gz' "$R/.calls" &&
  ok "geckodriver is fetched from its release tag v0.37.1" || bad "geckodriver URL: $(grep fetch "$R/.calls")"
grep -q '^changed: tool:geckodriver: geckodriver 0.37.1 installed: geckodriver$' "$W/out" && ok "apply installs geckodriver 0.37.1" || bad "geckodriver apply: $(cat "$W/out" "$W/err" | grep tool)"
printf '#!/bin/sh\necho "geckodriver 0.37.1 (0123abcdef01 2026-01-01 00:00 +0000)"\necho\necho "The source code of this program is available"\n' >"$R/srv/slot/cargo/bin/geckodriver"
run "$R" check
grep -q 'tool:geckodriver' "$W/out" && bad "a geckodriver whose first line names 0.37.1 is drift: $(grep tool "$W/out")" || ok "check reads the first line's name and version"
printf '#!/bin/sh\necho "geckodriver 0.36.0 (0123abcdef01 2025-01-01 00:00 +0000)"\n' >"$R/srv/slot/cargo/bin/geckodriver"
run "$R" check
grep -q "^drift: tool:geckodriver: .*'geckodriver 0.36.0', want geckodriver 0.37.1" "$W/out" && ok "check names a geckodriver of another release" || bad "geckodriver 0.36.0: $(grep tool "$W/out")"
run "$R" apply
# cargo-nextest and go-task, the tools consumers' CI pins. nextest's archive holds only the binary
# (its `--version` starts `cargo-nextest 0.9.143`); task's holds more files and its `--version` is the
# bare `3.54.0`, which check reads as `task 3.54.0`.
grep -qx 'cargo-nextest 0.9.143' "$W/pins" && ok "pins: cargo-nextest 0.9.143" || bad "pins: no cargo-nextest 0.9.143: $(cat "$W/pins")"
grep -qx 'task 3.54.0' "$W/pins" && ok "pins: task 3.54.0" || bad "pins: no task 3.54.0: $(cat "$W/pins")"
rm -f "$R/srv/slot/cargo/bin/cargo-nextest" "$R/srv/slot/cargo/bin/task" "$R/srv/slot/cargo/bin/ess" "$R/srv/slot/cargo/bin/cargo-about" "$R/srv/slot/cargo/bin/aep"
: >"$R/.calls"
run "$R" apply
grep -qx 'cargo +1.99.1 install --locked --force --git https://github.com/beyond10x/aep --rev af11af152de1732383a5edee0da723bb72d9acb4 aep-cli --root /srv/slot/cargo' "$R/.calls" &&
  ok "aep is built from the revision of its tag, on the pinned toolchain" || bad "aep install call: $(grep cargo "$R/.calls")"
grep -q '^changed: tool:aep: aep 0.80.0 built from af11af152de1732383a5edee0da723bb72d9acb4: aep$' "$W/out" && ok "apply builds aep 0.80.0" || bad "aep apply: $(cat "$W/out" "$W/err" | grep aep)"
grep -qx 'fetch https://github.com/beyond10x/ess/releases/download/0.99.2/ess-0.99.2-x86_64-unknown-linux-gnu.tar.gz' "$R/.calls" &&
  ok "ess is fetched from its release 0.99.2" || bad "ess URL: $(grep fetch "$R/.calls")"
grep -qx 'fetch https://github.com/EmbarkStudios/cargo-about/releases/download/0.9.1/cargo-about-0.9.1-x86_64-unknown-linux-musl.tar.gz' "$R/.calls" &&
  ok "cargo-about is fetched from its release 0.9.1" || bad "cargo-about URL: $(grep fetch "$R/.calls")"
grep -q '^changed: tool:ess: ess 0.99.2 installed: ess$' "$W/out" && ok "apply installs ess 0.99.2" || bad "ess apply: $(cat "$W/out" "$W/err")"
grep -qx 'fetch https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.143/cargo-nextest-0.9.143-x86_64-unknown-linux-gnu.tar.gz' "$R/.calls" &&
  ok "cargo-nextest is fetched from its release tag" || bad "nextest URL: $(grep fetch "$R/.calls")"
grep -qx 'fetch https://github.com/go-task/task/releases/download/v3.54.0/task_linux_amd64.tar.gz' "$R/.calls" &&
  ok "task is fetched from its release tag v3.54.0" || bad "task URL: $(grep fetch "$R/.calls")"
printf '#!/bin/sh\necho 3.54.0\n' >"$R/srv/slot/cargo/bin/task"
run "$R" check
grep -q 'tool:task' "$W/out" && bad "a task that prints the bare 3.54.0 is drift: $(grep tool "$W/out")" || ok "check reads a bare version as <binary> <version>"
printf '#!/bin/sh\necho 3.53.0\n' >"$R/srv/slot/cargo/bin/task"
run "$R" check
grep -q "^drift: tool:task: .*'task 3.53.0', want task 3.54.0" "$W/out" && ok "check names a task of another release" || bad "task 3.53.0: $(grep tool "$W/out")"
run "$R" apply

# 5h. Pinned archives (ARCHIVE_PINS): go and node at the release the workstation runs, in a
# root-owned /usr/local/<name>-<version>, linked into /usr/local/bin. check reads each link and the
# version of the first binary of the pin.
conf
R5=$W/r5
mkdir -p "$R5"
run "$R5" check
grep -q '^drift: archive:go: ' "$W/out" && grep -q '^drift: archive:node: ' "$W/out" && ok "check names the archive pins of a bare node" || bad "archive drift: $(grep archive "$W/out")"
: >"$R5/.calls"
run "$R5" apply
grep -qx 'fetch https://go.dev/dl/go1.27.1.linux-amd64.tar.gz' "$R5/.calls" && ok "go 1.27.1 is fetched from go.dev/dl" || bad "go URL: $(grep fetch "$R5/.calls")"
grep -qx 'fetch https://nodejs.org/dist/v22.23.3/node-v22.23.3-linux-x64.tar.gz' "$R5/.calls" && ok "node v22.23.3 is fetched from nodejs.org/dist" || bad "node URL: $(grep fetch "$R5/.calls")"
for l in go:go-1.27.1/bin/go gofmt:go-1.27.1/bin/gofmt node:node-v22.23.3/bin/node npm:node-v22.23.3/bin/npm npx:node-v22.23.3/bin/npx corepack:node-v22.23.3/bin/corepack; do
  [ "$(readlink "$R5/usr/local/bin/${l%%:*}")" = "/usr/local/${l#*:}" ] && ok "/usr/local/bin/${l%%:*} links to /usr/local/${l#*:}" || bad "link ${l%%:*}: '$(readlink "$R5/usr/local/bin/${l%%:*}")'"
done
[ "$("$R5/usr/local/go-1.27.1/bin/go" version)" = "go version go1.27.1 linux/amd64" ] && ok "go is extracted into /usr/local/go-1.27.1" || bad "go extract: $("$R5/usr/local/go-1.27.1/bin/go" version 2>&1)"
[ -z "$(find "$R5/usr/local/go-1.27.1" "$R5/usr/local/node-v22.23.3" -perm /022 ! -type l)" ] && ok "the extracted trees are not writable by group or others" || bad "writable: $(find "$R5/usr/local/go-1.27.1" "$R5/usr/local/node-v22.23.3" -perm /022 ! -type l | head -3)"
run "$R5" apply
run "$R5" check
[ $? -eq 0 ] && ok "after apply the archive pins have 0 drift" || bad "archive pins after apply: $(cat "$W/out")"
rm -f "$R5/usr/local/bin/node"
printf '#!/bin/sh\necho "go version go1.26.0 linux/amd64"\n' >"$R5/usr/local/go-1.27.1/bin/go"
run "$R5" check
grep -q "^drift: archive:node: /usr/local/bin/node is 'missing', want a link to /usr/local/node-v22.23.3/bin/node" "$W/out" && ok "check names a missing link" || bad "missing link: $(grep archive "$W/out")"
grep -q "^drift: archive:go: /usr/local/go-1.27.1/bin/go says 'go version go1.26.0 linux/amd64', want go1.27.1" "$W/out" && ok "check names a version that differs" || bad "version drift: $(grep archive "$W/out")"
run "$R5" apply
run "$R5" check
[ $? -eq 0 ] && ok "apply restores the link and the release" || bad "archive restore: $(cat "$W/out" "$W/err")"
rm -rf "$R5/usr/local/go-1.27.1" "$R5/usr/local/bin/gofmt" "$R5/usr/local/node-v22.23.3"
: >"$R5/.state-tool-fetch-fails"
run "$R5" apply
st=$?
rm -f "$R5/.state-tool-fetch-fails"
[ $st -eq 1 ] && grep -qx 'failed: archive:go: download or checksum of go 1.27.1' "$W/err" && ok "a failed archive download: apply exits 1 and names it" || bad "failed archive download: exit $st $(grep archive "$W/err")"
grep -q '^changed: archive' "$W/out" && bad "a failed archive install is reported as changed" || ok "a failed archive install is not reported as changed"
run "$R5" apply
run "$R5" check
[ $? -eq 0 ] && ok "after the download works: archive pins 0 drift" || bad "archive after the download: $(cat "$W/out")"

# typescript, prettier and esbuild are archives of the same kind (the npm tarballs of the releases
# the consumers' test lanes read): each binary is linked as /usr/local/bin/<name>, prettier's
# bin/prettier.cjs as `prettier`, and the script is executable. pnpm is one more of them, and yarn
# (one script, bin/yarn.js, linked as `yarn` and as `yarnpkg`); helm is an archive with one binary,
# linked the same way.
for l in tsc:typescript-6.0.3/bin/tsc tsserver:typescript-6.0.3/bin/tsserver prettier:prettier-3.8.1/bin/prettier.cjs esbuild:esbuild-0.28.2/bin/esbuild pnpm:pnpm-11.9.0/bin/pnpm.mjs pnpx:pnpm-11.9.0/bin/pnpx.mjs yarn:yarn-1.22.22/bin/yarn.js yarnpkg:yarn-1.22.22/bin/yarn.js helm:helm-v3.8.1/helm; do
  [ "$(readlink "$R5/usr/local/bin/${l%%:*}")" = "/usr/local/${l#*:}" ] && ok "/usr/local/bin/${l%%:*} links to /usr/local/${l#*:}" || bad "link ${l%%:*}: '$(readlink "$R5/usr/local/bin/${l%%:*}")'"
done
grep -qx 'fetch https://registry.npmjs.org/yarn/-/yarn-1.22.22.tgz' "$R5/.calls" && [ -x "$R5/usr/local/yarn-1.22.22/bin/yarn.js" ] && [ "$(YARN_IGNORE_PATH=1 "$R5/usr/local/yarn-1.22.22/bin/yarn.js" --version 2>&1)" = "1.22.22" ] && ok "yarn 1.22.22 is the npm tarball of that release, and bin/yarn.js is executable and answers it" || bad "yarn: $(grep 'fetch.*yarn' "$R5/.calls") $(ls -l "$R5/usr/local/yarn-1.22.22/bin/yarn.js" 2>&1)"
# yarn 1 runs the script that `yarn-path` of a .yarnrc names (the build user's, or one in the
# working directory) and prints that one's version; with YARN_IGNORE_PATH=1 it answers itself.
# The stand-in does the same: without the variable it answers as another release would. check
# and apply read the version with the variable (0 drift above), or every apply would install again.
[ "$("$R5/usr/local/yarn-1.22.22/bin/yarn.js" --version 2>&1)" != "1.22.22" ] && ok "yarn's version is read with YARN_IGNORE_PATH=1: a yarn-path of a .yarnrc does not answer for it" || bad "the yarn stand-in answers 1.22.22 without YARN_IGNORE_PATH: the variable is not tested"
# The pin itself. The SHA-256 is the one of the tarball on registry.npmjs.org; an npm tarball has
# one top directory, package/, which is stripped. The line then goes through the real branch of
# archive_fetch with a tarball of that shape (curl is a stand-in that serves it, with its own
# SHA-256): bin/yarn.js is at the top of the tree, executable.
ypin=$(grep '^yarn ' "$P")
[ "$ypin" = 'yarn 1.22.22 c17d3797fb9a9115bf375e31bfd30058cac6bc9c3b8807a3d8cb2094794b51ca https://registry.npmjs.org/yarn/-/yarn-1.22.22.tgz 1 bin/yarn.js:yarn bin/yarn.js:yarnpkg' ] && ok "the yarn pin: release 1.22.22, the SHA-256 of its npm tarball, one leading directory stripped" || bad "yarn pin: $ypin"
mkdir -p "$W/ysrc/package/bin" "$W/yfetch"
printf '#!/usr/bin/env node\n' >"$W/ysrc/package/bin/yarn.js"
tar -czf "$W/ysrc.tgz" -C "$W/ysrc" package
ytree=$(
  eval "$(sed -n '/^archive_src() /p; /^archive_fetch() {$/,/^}$/p' "$P")"
  fake_fetch() { false; }
  curl() { cp "$W/ysrc.tgz" "$2"; }
  # shellcheck disable=SC2086
  set -- $ypin
  ysum=$(sha256sum "$W/ysrc.tgz" | cut -d' ' -f1)
  archive_fetch "$W/yfetch" "$1" "$2" "$ysum" "$4" "$5" "$6" "$7" 2>&1
)
[ "$ytree" = "$W/yfetch/tree" ] && [ -x "$W/yfetch/tree/bin/yarn.js" ] && [ ! -e "$W/yfetch/tree/package" ] && ok "the yarn pin through the real unpack: package/ is stripped, bin/yarn.js is executable" || bad "yarn unpack: '$ytree' $(find "$W/yfetch" -type f 2>&1 | head -5)"
[ "$("$R5/usr/local/prettier-3.8.1/bin/prettier.cjs" --version)" = "3.8.1" ] && [ "$("$R5/usr/local/esbuild-0.28.2/bin/esbuild" --version)" = "0.28.2" ] && ok "prettier and esbuild answer their release (run by their tree path: a link names the real /usr/local)" || bad "tool versions: $("$R5/usr/local/prettier-3.8.1/bin/prettier.cjs" --version 2>&1) $("$R5/usr/local/esbuild-0.28.2/bin/esbuild" --version 2>&1)"
[ -x "$R5/usr/local/prettier-3.8.1/bin/prettier.cjs" ] && [ ! -L "$R5/usr/local/prettier-3.8.1/bin/prettier.cjs" ] && ok "a linked script the tarball ships without the execute bit is executable after apply" || bad "prettier.cjs mode: $(ls -l "$R5/usr/local/prettier-3.8.1/bin/prettier.cjs" 2>&1)"
rm -f "$R5/usr/local/bin/prettier"
rm -rf "$R5/usr/local/esbuild-0.28.2"
run "$R5" check
grep -q "^drift: archive:prettier: /usr/local/bin/prettier is 'missing', want a link to /usr/local/prettier-3.8.1/bin/prettier.cjs" "$W/out" && ok "check names a missing prettier link" || bad "prettier drift: $(grep archive "$W/out")"
grep -q "^drift: archive:esbuild: " "$W/out" && ok "check names a missing esbuild tree" || bad "esbuild drift: $(grep archive "$W/out")"
: >"$R5/.calls"
run "$R5" apply
grep -qx 'fetch https://registry.npmjs.org/prettier/-/prettier-3.8.1.tgz' "$R5/.calls" && grep -qx 'fetch https://registry.npmjs.org/@esbuild/linux-x64/-/linux-x64-0.28.2.tgz' "$R5/.calls" && ok "apply fetches the npm tarballs of prettier and esbuild" || bad "npm URLs: $(grep fetch "$R5/.calls")"
run "$R5" check
[ $? -eq 0 ] && ok "after apply the npm tools have 0 drift" || bad "npm tools after apply: $(cat "$W/out")"

# yq is the Go one of mikefarah: the release tarball names its binary yq_linux_amd64 (no leading
# directory), linked as /usr/local/bin/yq; `yq --version` has the release as its last word.
[ "$(readlink "$R5/usr/local/bin/yq")" = "/usr/local/yq-v4.30.8/yq_linux_amd64" ] && ok "/usr/local/bin/yq links to /usr/local/yq-v4.30.8/yq_linux_amd64" || bad "yq link: '$(readlink "$R5/usr/local/bin/yq")'"
[ -x "$R5/usr/local/yq-v4.30.8/yq_linux_amd64" ] && [ "$("$R5/usr/local/yq-v4.30.8/yq_linux_amd64" --version)" = "v4.30.8" ] && ok "yq answers v4.30.8 (run by its tree path)" || bad "yq: $("$R5/usr/local/yq-v4.30.8/yq_linux_amd64" --version 2>&1)"
rm -f "$R5/usr/local/bin/yq"
rm -rf "$R5/usr/local/yq-v4.30.8"
run "$R5" check
grep -q "^drift: archive:yq: /usr/local/bin/yq is 'missing', want a link to /usr/local/yq-v4.30.8/yq_linux_amd64" "$W/out" && ok "check names a missing yq" || bad "yq drift: $(grep archive "$W/out")"
: >"$R5/.calls"
run "$R5" apply
grep -qx 'fetch https://github.com/mikefarah/yq/releases/download/v4.30.8/yq_linux_amd64.tar.gz' "$R5/.calls" && ok "apply fetches the yq release tarball" || bad "yq URL: $(grep fetch "$R5/.calls")"
run "$R5" check
[ $? -eq 0 ] && ok "after apply yq has 0 drift" || bad "yq after apply: $(cat "$W/out")"

# GNU make 4.4.1 from its source tarball: the node's make 4.3 cannot parse the pool's `fifo:`
# jobserver. The binary is in /usr/local/make-4.4.1/bin, linked as make and gmake.
for l in make gmake; do
  [ "$(readlink "$R5/usr/local/bin/$l")" = "/usr/local/make-4.4.1/bin/make" ] && ok "/usr/local/bin/$l links to /usr/local/make-4.4.1/bin/make" || bad "$l link: '$(readlink "$R5/usr/local/bin/$l")'"
done
[ "$("$R5/usr/local/make-4.4.1/bin/make" --version)" = "GNU Make 4.4.1" ] && ok "make 4.4.1 answers its release (run by its tree path)" || bad "make: $("$R5/usr/local/make-4.4.1/bin/make" --version 2>&1)"
rm -f "$R5/usr/local/bin/gmake"
printf '#!/bin/sh\necho "GNU Make 4.3"\n' >"$R5/usr/local/make-4.4.1/bin/make"
run "$R5" check
grep -q "^drift: make: /usr/local/bin/gmake is 'missing', want a link to /usr/local/make-4.4.1/bin/make" "$W/out" && ok "check names a missing gmake link" || bad "gmake drift: $(grep make "$W/out")"
grep -q "^drift: make: /usr/local/make-4.4.1/bin/make says 'GNU Make 4.3', want GNU Make 4.4.1" "$W/out" && ok "check names a make of another release" || bad "make 4.3 drift: $(grep make "$W/out")"
: >"$R5/.calls"
run "$R5" apply
grep -qx 'fetch https://ftp.gnu.org/gnu/make/make-4.4.1.tar.gz' "$R5/.calls" && ok "apply fetches the make source tarball" || bad "make URL: $(grep fetch "$R5/.calls")"
grep -q '^changed: make: GNU Make 4.4.1 built, linked as make and gmake in /usr/local/bin$' "$W/out" && ok "apply builds make 4.4.1" || bad "make apply: $(cat "$W/out" "$W/err" | grep make)"
run "$R5" check
[ $? -eq 0 ] && ok "after apply make has 0 drift" || bad "make after apply: $(cat "$W/out")"
rm -f "$R5/usr/local/bin/make"
: >"$R5/.calls"
run "$R5" apply
grep -q '^fetch .*make-' "$R5/.calls" && bad "a missing link fetched the make tarball again" || ok "a missing link alone does not fetch or build"
grep -q '^changed: make: linked make and gmake in /usr/local/bin to /usr/local/make-4.4.1/bin/make$' "$W/out" && ok "a missing link alone is relinked" || bad "make relink: $(cat "$W/out" "$W/err" | grep make)"
rm -f "$R5/usr/local/bin/make"
printf '#!/bin/sh\necho "real file"\n' >"$R5/usr/local/bin/make"
run "$R5" apply
st=$?
grep -q '^failed: make: /usr/local/bin/make is a real file; not replaced$' "$W/err" && ok "a real file at /usr/local/bin/make is not replaced" || bad "make real file: $(cat "$W/err")"
[ ! -L "$R5/usr/local/bin/make" ] && [ "$st" -eq 1 ] && ok "the real file stays and apply exits 1" || bad "real file replaced or exit $st"
rm -f "$R5/usr/local/bin/make"
printf '#!/bin/sh\necho "GNU Make 4.3"\n' >"$R5/usr/local/make-4.4.1/bin/make"
printf 'stale\n' >"$R5/usr/local/make-4.4.1/bin/make.slot-new"
: >"$R5/.state-tool-fetch-fails"
run "$R5" apply
st=$?
rm -f "$R5/.state-tool-fetch-fails"
grep -q '^failed: make: download, checksum or build of make 4.4.1' "$W/err" && ok "a failed make build is named" || bad "make failure not named: $(cat "$W/err")"
[ "$st" -eq 1 ] && ok "a failed make build exits 1" || bad "failed make build exit $st"
grep -q '^changed: make' "$W/out" && bad "a failed make build is reported as changed" || ok "a failed make build is not reported as changed"
[ "$("$R5/usr/local/make-4.4.1/bin/make" --version)" = "GNU Make 4.3" ] && ok "a failed make build leaves the binary as it was" || bad "make binary changed by a failed build"
run "$R5" apply
[ ! -e "$R5/usr/local/make-4.4.1/bin/make.slot-new" ] && ok "a leftover make.slot-new does not survive the next apply" || bad "make.slot-new left"
run "$R5" check
[ $? -eq 0 ] && ok "make has 0 drift after the repair" || bad "make after repair: $(cat "$W/out")"

# musl 1.2.6 from its source tarball (MUSL_PIN): Amazon Linux 2023 packages no musl-gcc, and a
# build that names it as the C compiler of the musl target stopped with `failed to find tool
# "musl-gcc"`. The tree is /usr/local/musl-1.2.6, its wrapper linked as /usr/local/bin/musl-gcc.
MU=/usr/local/musl-1.2.6
musl_url=https://musl.libc.org/releases/musl-1.2.6.tar.gz
mlines() { cat "$W/out" "$W/err" | grep 'musl' | head -5; }
[ "$(sed -n "s/^MUSL_PIN='\(.*\)'\$/\1/p" "$P")" = "musl 1.2.6 d585fd3b613c66151fc3249e8ed44f77020cb5e6c1e635a616d3f9f82460512a $musl_url" ] && ok "the musl pin: release 1.2.6, its SHA-256, the tarball of musl.libc.org" || bad "musl pin: $(grep '^MUSL_PIN' "$P")"
[ "$(readlink "$R5/usr/local/bin/musl-gcc")" = "$MU/bin/musl-gcc" ] && ok "/usr/local/bin/musl-gcc links to $MU/bin/musl-gcc" || bad "musl-gcc link: '$(readlink "$R5/usr/local/bin/musl-gcc")'"
[ -x "$R5$MU/bin/musl-gcc" ] && [ -f "$R5$MU/lib/musl-gcc.specs" ] && [ -f "$R5$MU/lib/libc.a" ] && [ -f "$R5$MU/include/stdio.h" ] && [ -z "$(find "$R5$MU" -perm /022 ! -type l)" ] && [ ! -e "$R5$MU.slot-new" ] && ok "the musl tree has the wrapper, its specs, the static library and the headers, nothing writable by group or others, no .slot-new" || bad "musl tree: $(ls -lR "$R5$MU" 2>&1 | head -20)"
[ "$("$R5$MU/lib/libc.so" 2>&1 | sed -n 's/^Version //p')" = 1.2.6 ] && ok "the loader of the musl tree names release 1.2.6" || bad "musl loader: $("$R5$MU/lib/libc.so" 2>&1 | head -3)"
# The build runs once: a later apply with the release present fetches and builds nothing.
: >"$R5/.calls"
run "$R5" apply
st=$?
[ $st -eq 0 ] && ! grep -q "^fetch $musl_url" "$R5/.calls" && ! grep -q '^changed: musl\|^drift: musl' "$W/out" && ok "a second apply with musl 1.2.6 present fetches and builds nothing" || bad "second apply and musl: exit $st $(grep musl "$R5/.calls") $(mlines)"
# A missing link alone is set again without a download.
rm -f "$R5/usr/local/bin/musl-gcc"
run "$R5" check
st=$?
[ $st -eq 1 ] && grep -qx "drift: musl: /usr/local/bin/musl-gcc is 'missing', want a link to $MU/bin/musl-gcc" "$W/out" && ok "check names a missing musl-gcc link" || bad "musl link drift: exit $st $(mlines)"
: >"$R5/.calls"
run "$R5" apply
grep -qx "changed: musl: linked musl-gcc in /usr/local/bin to $MU/bin/musl-gcc" "$W/out" && ! grep -q "^fetch $musl_url" "$R5/.calls" && [ "$(readlink "$R5/usr/local/bin/musl-gcc")" = "$MU/bin/musl-gcc" ] && ok "a missing musl-gcc link alone is set again without a download" || bad "musl relink: $(grep musl "$R5/.calls") $(mlines)"
# A link to another place and a tree of another release: both are named, and apply replaces the
# tree whole.
ln -sfn /usr/bin/gcc "$R5/usr/local/bin/musl-gcc"
printf '#!/bin/sh\necho "musl libc (x86_64)" >&2\necho "Version 1.2.5" >&2\nexit 1\n' >"$R5$MU/lib/libc.so"
: >"$R5$MU/lib/file-of-the-old-release"
run "$R5" check
grep -qx "drift: musl: /usr/local/bin/musl-gcc is '/usr/bin/gcc', want a link to $MU/bin/musl-gcc" "$W/out" && ok "check names a musl-gcc link to another place" || bad "musl wrong link: $(mlines)"
grep -qx "drift: musl: $MU/lib/libc.so says 'Version 1.2.5', want Version 1.2.6" "$W/out" && ok "check names a musl of another release" || bad "musl 1.2.5 drift: $(mlines)"
: >"$R5/.calls"
run "$R5" apply
st=$?
[ $st -eq 0 ] && grep -qx "fetch $musl_url" "$R5/.calls" && grep -qx "changed: musl: musl 1.2.6 built in $MU, linked as musl-gcc in /usr/local/bin" "$W/out" && ok "apply fetches the musl source tarball and builds release 1.2.6" || bad "musl apply: exit $st $(grep musl "$R5/.calls") $(mlines)"
[ ! -e "$R5$MU/lib/file-of-the-old-release" ] && [ ! -e "$R5$MU.slot-new" ] && [ ! -e "$R5$MU.slot-old" ] && [ "$(readlink "$R5/usr/local/bin/musl-gcc")" = "$MU/bin/musl-gcc" ] && ok "the tree of the other release is replaced whole, the link set, no .slot-new or .slot-old left" || bad "musl replace: $(ls -a "$R5/usr/local" "$R5$MU/lib" 2>&1 | tr '\n' ' ')"
run "$R5" check
[ $? -eq 0 ] && ok "after apply musl has 0 drift" || bad "musl after apply: $(cat "$W/out")"
# A wrapper that answers nothing is drift. A real file at the link's name is left alone, and
# nothing is fetched for it.
cp "$R5$MU/bin/musl-gcc" "$W/musl-gcc.standin"
printf '#!/bin/sh\nexit 1\n' >"$R5$MU/bin/musl-gcc"
rm -f "$R5/usr/local/bin/musl-gcc"
printf '#!/bin/sh\necho "real file"\n' >"$R5/usr/local/bin/musl-gcc"
run "$R5" check
grep -qx "drift: musl: $MU/bin/musl-gcc --version answers nothing" "$W/out" && ok "check names a musl-gcc that does not answer" || bad "musl no answer: $(mlines)"
: >"$R5/.calls"
run "$R5" apply
st=$?
[ $st -eq 1 ] && grep -qx 'failed: musl: /usr/local/bin/musl-gcc is a real file; not replaced' "$W/err" && [ ! -L "$R5/usr/local/bin/musl-gcc" ] && ! grep -q "^fetch $musl_url" "$R5/.calls" && ok "a real file at /usr/local/bin/musl-gcc is not replaced, apply exits 1 and fetches nothing" || bad "musl real file: exit $st $(grep musl "$R5/.calls") $(mlines)"
rm -f "$R5/usr/local/bin/musl-gcc"
# A failed download or build is named, exits 1 and leaves the tree as it was.
mkdir -p "$R5$MU.slot-new"
: >"$R5/.state-tool-fetch-fails"
run "$R5" apply
st=$?
rm -f "$R5/.state-tool-fetch-fails"
[ $st -eq 1 ] && grep -q '^failed: musl: download, checksum or build of musl 1.2.6' "$W/err" && ! grep -q '^changed: musl' "$W/out" && ok "a failed musl build is named, exits 1 and is not reported as changed" || bad "musl failure: exit $st $(mlines)"
[ "$(cat "$R5$MU/bin/musl-gcc")" = "$(printf '#!/bin/sh\nexit 1\n')" ] && [ ! -e "$R5/usr/local/bin/musl-gcc" ] && ok "a failed musl build leaves the tree as it was and sets no link" || bad "musl tree changed by a failed build: $(cat "$R5$MU/bin/musl-gcc") $(readlink "$R5/usr/local/bin/musl-gcc")"
run "$R5" apply
[ ! -e "$R5$MU.slot-new" ] && cmp -s "$R5$MU/bin/musl-gcc" "$W/musl-gcc.standin" && ok "the next apply builds musl again; a leftover .slot-new does not survive it" || bad "musl repair: $(ls -a "$R5/usr/local" | tr '\n' ' ') $(mlines)"
run "$R5" check
[ $? -eq 0 ] && ok "musl has 0 drift after the repair" || bad "musl after repair: $(cat "$W/out")"
# A pin of another release: its own directory, and the link goes there. The directory of the
# release before stays, like the one of make and of an archive.
MP9="musl 9.9.9 $(printf '%064d' 9) https://musl.libc.org/releases/musl-9.9.9.tar.gz"
: >"$R5/.calls"
NODE_TEST_MUSL_PIN=$MP9 NODE_TEST=1 NODE_ROOT=$R5 sh "$P" check "$W/node.conf" >"$W/out" 2>"$W/err"
grep -qx "drift: musl: /usr/local/bin/musl-gcc is '$MU/bin/musl-gcc', want a link to /usr/local/musl-9.9.9/bin/musl-gcc" "$W/out" && grep -qx "drift: musl: /usr/local/musl-9.9.9/lib/libc.so says 'missing', want Version 9.9.9" "$W/out" && ok "check names the link and the missing tree of a pin of another release" || bad "musl pin change drift: $(mlines)"
NODE_TEST_MUSL_PIN=$MP9 NODE_TEST=1 NODE_ROOT=$R5 sh "$P" apply "$W/node.conf" >"$W/out" 2>"$W/err"
st=$?
[ $st -eq 0 ] && grep -qx 'fetch https://musl.libc.org/releases/musl-9.9.9.tar.gz' "$R5/.calls" && [ "$(readlink "$R5/usr/local/bin/musl-gcc")" = /usr/local/musl-9.9.9/bin/musl-gcc ] && [ "$("$R5/usr/local/musl-9.9.9/lib/libc.so" 2>&1 | sed -n 's/^Version //p')" = 9.9.9 ] && [ -d "$R5$MU" ] && ok "a pin of another release is built in its own directory and the link goes there" || bad "musl pin change: exit $st $(grep musl "$R5/.calls") $(mlines)"
: >"$R5/.calls"
run "$R5" apply
st=$?
[ $st -eq 0 ] && ! grep -q '^fetch .*musl' "$R5/.calls" && [ "$(readlink "$R5/usr/local/bin/musl-gcc")" = "$MU/bin/musl-gcc" ] && ok "back at the pin, the link returns to the tree that is there without a download" || bad "musl pin back: exit $st $(grep musl "$R5/.calls") $(mlines)"
rm -rf "$R5/usr/local/musl-9.9.9"

# dash 0.5.12 from its source tarball (DASH_PIN): the shell that is `sh` on the forge's runner
# image, so a shell test of a job can run under it. The binary is in /usr/local/dash-0.5.12/bin,
# linked as dash and never as sh. dash prints no version: the tree carries the SHA-256 of the
# tarball it was built from, and the binary must run a command.
DA=/usr/local/dash-0.5.12
DS=6a474ac46e8b0b32916c4c60df694c82058d3297d8b385b74508030ca4a8f28a
dlines() { cat "$W/out" "$W/err" | grep 'dash' | tr '\n' ' '; }
[ "$(readlink "$R5/usr/local/bin/dash")" = "$DA/bin/dash" ] && ok "/usr/local/bin/dash links to $DA/bin/dash" || bad "dash link: '$(readlink "$R5/usr/local/bin/dash")'"
[ "$(cat "$R5$DA/.slot-sha256" 2>/dev/null)" = "$DS" ] && ok "the dash tree names the SHA-256 of its tarball" || bad "dash mark: '$(cat "$R5$DA/.slot-sha256" 2>&1)'"
[ "$("$R5$DA/bin/dash" -c 'echo $((6 * 7))' 2>&1)" = 42 ] && ok "the installed dash runs a command" || bad "dash -c: $("$R5$DA/bin/dash" -c 'echo $((6 * 7))' 2>&1)"
[ ! -e "$R5/usr/local/bin/sh" ] && [ ! -L "$R5/usr/local/bin/sh" ] && ok "dash is not linked as sh" || bad "/usr/local/bin/sh exists"
sh "$here/toolchains.sh" "$R5" "$W/norust" 2>/dev/null | grep '^tool dash' >"$W/out2"
[ "$(cat "$W/out2")" = "tool dash 0.5.12" ] && ok "toolchains.sh: one line, tool dash 0.5.12" || bad "tool dash line: '$(cat "$W/out2")'"
rm -f "$R5/usr/local/bin/dash"
printf 'abc\n' >"$R5$DA/.slot-sha256"
run "$R5" check
grep -qx "drift: dash: /usr/local/bin/dash is 'missing', want a link to $DA/bin/dash" "$W/out" && ok "check names a missing dash link" || bad "dash link drift: $(dlines)"
grep -qx "drift: dash: $DA is of the tarball 'abc', want $DS" "$W/out" && ok "check names a dash tree of another tarball" || bad "dash mark drift: $(dlines)"
printf '%s\n' "$DS" >"$R5$DA/.slot-sha256"
printf '#!/bin/sh\nexit 1\n' >"$R5$DA/bin/dash"
run "$R5" check
grep -qx "drift: dash: $DA/bin/dash does not run a command (it answers '', want 42)" "$W/out" && ok "check names a dash that runs no command" || bad "dash run drift: $(dlines)"
: >"$R5/.calls"
run "$R5" apply
grep -qx 'fetch http://gondor.apana.org.au/~herbert/dash/files/dash-0.5.12.tar.gz' "$R5/.calls" && ok "apply fetches the dash source tarball" || bad "dash URL: $(grep fetch "$R5/.calls" | tr '\n' ' ')"
grep -q '^changed: dash: dash 0.5.12 built, linked as dash in /usr/local/bin$' "$W/out" && ok "apply builds dash 0.5.12" || bad "dash apply: $(dlines)"
run "$R5" check
[ $? -eq 0 ] && ok "after apply dash has 0 drift" || bad "dash after apply: $(cat "$W/out")"
rm -f "$R5/usr/local/bin/dash"
: >"$R5/.calls"
run "$R5" apply
grep -q '^fetch .*dash-' "$R5/.calls" && bad "a missing link fetched the dash tarball again" || ok "a missing dash link alone does not fetch or build"
grep -q "^changed: dash: linked dash in /usr/local/bin to $DA/bin/dash\$" "$W/out" && ok "a missing dash link alone is relinked" || bad "dash relink: $(dlines)"
rm -f "$R5/usr/local/bin/dash"
printf '#!/bin/sh\necho "real file"\n' >"$R5/usr/local/bin/dash"
run "$R5" apply
st=$?
grep -q '^failed: dash: /usr/local/bin/dash is a real file; not replaced$' "$W/err" && [ ! -L "$R5/usr/local/bin/dash" ] && [ "$st" -eq 1 ] && ok "a real file at /usr/local/bin/dash is not replaced, and apply exits 1" || bad "dash real file: exit $st $(dlines)"
rm -f "$R5/usr/local/bin/dash"
printf '#!/bin/sh\nexit 1\n' >"$R5$DA/bin/dash"
printf 'stale\n' >"$R5$DA/bin/dash.slot-new"
: >"$R5/.state-tool-fetch-fails"
run "$R5" apply
st=$?
rm -f "$R5/.state-tool-fetch-fails"
grep -q '^failed: dash: download, checksum or build of dash 0.5.12' "$W/err" && [ "$st" -eq 1 ] && ! grep -q '^changed: dash' "$W/out" && ok "a failed dash build is named, exits 1 and is not reported as changed" || bad "dash failure: exit $st $(dlines)"
[ "$(cat "$R5$DA/bin/dash")" = "$(printf '#!/bin/sh\nexit 1\n')" ] && [ ! -e "$R5/usr/local/bin/dash" ] && ok "a failed dash build leaves the binary as it was and sets no link" || bad "dash binary changed by a failed build"
run "$R5" apply
[ ! -e "$R5$DA/bin/dash.slot-new" ] && ok "a leftover dash.slot-new does not survive the next apply" || bad "dash.slot-new left"
run "$R5" check
[ $? -eq 0 ] && ok "dash has 0 drift after the repair" || bad "dash after repair: $(cat "$W/out")"
# A pin of another release: its own directory, and the link goes there.
DP9="dash 9.9.9 $(printf '%064d' 9) http://gondor.apana.org.au/~herbert/dash/files/dash-9.9.9.tar.gz"
: >"$R5/.calls"
NODE_TEST_DASH_PIN=$DP9 NODE_TEST=1 NODE_ROOT=$R5 sh "$P" apply "$W/node.conf" >"$W/out" 2>"$W/err"
st=$?
[ $st -eq 0 ] && grep -qx 'fetch http://gondor.apana.org.au/~herbert/dash/files/dash-9.9.9.tar.gz' "$R5/.calls" && [ "$(readlink "$R5/usr/local/bin/dash")" = /usr/local/dash-9.9.9/bin/dash ] && [ -x "$R5$DA/bin/dash" ] && ok "a pin of another release is built into its own directory and the link moves; the tree before stays" || bad "dash 9.9.9: exit $st $(dlines)"
sh "$here/toolchains.sh" "$R5" "$W/norust" 2>/dev/null | grep '^tool dash' >"$W/out2"
[ "$(cat "$W/out2")" = "tool dash 9.9.9" ] && ok "toolchains.sh reads the release from the tree the link names" || bad "tool dash line: '$(cat "$W/out2")'"
: >"$R5/.calls"
run "$R5" apply
st=$?
[ $st -eq 0 ] && ! grep -q '^fetch .*dash' "$R5/.calls" && [ "$(readlink "$R5/usr/local/bin/dash")" = "$DA/bin/dash" ] && ok "back at the pin, the dash link returns to the tree that is there without a download" || bad "dash pin back: exit $st $(dlines)"
rm -rf "$R5/usr/local/dash-9.9.9"
# toolchains.sh for a dash that is no build of the pin: a tree without the tarball's SHA-256
# prints `missing`; a package's dash prints the release its package manager names, without the
# package revision; a machine without dash prints no line.
TD=$W/tcd
mkdir -p "$TD/a/usr/local/bin" "$TD/a/usr/local/dash-0.5.12/bin" "$TD/b/usr/bin" "$TD/c/usr/bin" "$TD/pm"
printf '#!/bin/sh\nexit 0\n' >"$TD/a/usr/local/dash-0.5.12/bin/dash"
printf '#!/bin/sh\nexit 0\n' >"$TD/b/usr/bin/dash"
chmod 755 "$TD/a/usr/local/dash-0.5.12/bin/dash" "$TD/b/usr/bin/dash"
ln -s /usr/local/dash-0.5.12/bin/dash "$TD/a/usr/local/bin/dash"
for pm in pacman dpkg-query rpm; do
  printf '#!/bin/sh\n[ -s "%s/pm/%s.out" ] || exit 1\ncat "%s/pm/%s.out"\n' "$TD" "$pm" "$TD" "$pm" >"$TD/pm/$pm"
  chmod 755 "$TD/pm/$pm"
done
tcd() { PATH=$TD/pm:$PATH sh "$here/toolchains.sh" "$TD/$1" "$W/norust" 2>/dev/null | grep '^tool dash' | tr '\n' ','; }
[ "$(tcd a)" = "tool dash missing," ] && ok "toolchains.sh: a dash tree without the tarball's SHA-256 prints missing" || bad "tool dash without mark: '$(tcd a)'"
[ "$(tcd b)" = "tool dash missing," ] && ok "toolchains.sh: a dash that no package manager knows prints missing" || bad "tool dash unknown: '$(tcd b)'"
echo 'dash 0.5.12-1' >"$TD/pm/pacman.out"
[ "$(tcd b)" = "tool dash 0.5.12," ] && ok "toolchains.sh: a package's dash prints the release pacman names" || bad "tool dash pacman: '$(tcd b)'"
rm -f "$TD/pm/pacman.out"
echo '0.5.12-12ubuntu3' >"$TD/pm/dpkg-query.out"
[ "$(tcd b)" = "tool dash 0.5.12," ] && ok "toolchains.sh: a package's dash prints the release dpkg names" || bad "tool dash dpkg: '$(tcd b)'"
rm -f "$TD/pm/dpkg-query.out"
echo '0.5.12' >"$TD/pm/rpm.out"
[ "$(tcd b)" = "tool dash 0.5.12," ] && ok "toolchains.sh: a package's dash prints the release rpm names" || bad "tool dash rpm: '$(tcd b)'"
[ -z "$(tcd c)" ] && ok "toolchains.sh: no dash line on a machine without dash" || bad "tool dash without dash: '$(tcd c)'"

# The build user's home trees (HOME_PINS): the ess releases a repository pins by path, and the
# @types/node of ESS_TYPES_NODE with its dependency.
HP=$R5/var/lib/builder/.local/share
for v in 0.97.4 0.98.0 0.99.1 0.99.4; do
  [ "$("$HP/ess-versions/$v/ess" --version)" = "ess $v" ] && ok "ess $v is in ~/.local/share/ess-versions/$v/ess" || bad "ess $v: $("$HP/ess-versions/$v/ess" --version 2>&1)"
done
grep -q '"version": "26.3.0"' "$HP/ess-node/node_modules/@types/node/package.json" && grep -q '"version": "8.3.0"' "$HP/ess-node/node_modules/undici-types/package.json" && ok "@types/node 26.3.0 and undici-types 8.3.0 are in ess-node/node_modules" || bad "ess-node: $(ls -R "$HP/ess-node" 2>&1 | head -5)"
for rv in 1254:158.0.8127.18 1234:151.0.7922.34; do
  rd=$R5/var/lib/builder/.cache/ms-playwright/chromium_headless_shell-${rv%%:*}
  [ "$("$rd/chrome-headless-shell-linux64/chrome-headless-shell" --version)" = "Google Chrome for Testing ${rv#*:}" ] && [ -e "$rd/INSTALLATION_COMPLETE" ] && [ -e "$rd/DEPENDENCIES_VALIDATED" ] && ok "Playwright chromium_headless_shell-${rv%%:*} is ${rv#*:} with its two markers" || bad "browser ${rv%%:*}: $(ls -a "$rd" 2>&1)"
done
AEPF=$R5/var/lib/builder/.cache/sdc/aep/0.80.0/x86_64-unknown-linux-gnu/aep
[ -x "$AEPF" ] && [ "$("$AEPF" --version)" = "aep 0.80.0" ] && ok "the pinned aep (stub download) is at ~/.cache/sdc/aep/0.80.0/x86_64-unknown-linux-gnu/aep, where runbook's driver looks; the 0-drift check after apply below covers it too" || bad "sdc aep: $(ls -l "$AEPF" 2>&1)"
rm -rf "$HP/ess-versions/0.99.1"
printf '#!/bin/sh\necho "ess 0.1.0"\n' >"$HP/ess-versions/0.98.0/ess"
rm -f "$HP/ess-node/node_modules/undici-types/package.json"
run "$R5" check
grep -q "^drift: home:ess@0.99.1: .*ess-versions/0.99.1 is 'missing', want ess 0.99.1" "$W/out" && grep -q "^drift: home:ess@0.98.0: .*ess-versions/0.98.0 is 'ess 0.1.0', want ess 0.98.0" "$W/out" && grep -q "^drift: home:undici-types@8.3.0: " "$W/out" && ok "check names a missing ess, an ess of another release and a missing package" || bad "home drift: $(grep home "$W/out")"
grep -q 'home:ess@0.97.4\|home:ess@0.99.4\|home:@types' "$W/out" && bad "check names a tree that is right: $(grep home "$W/out")" || ok "check names only the trees that differ"
: >"$R5/.calls"
run "$R5" apply
[ "$(grep -c '^fetch ' "$R5/.calls")" = 3 ] && grep -qx 'fetch https://github.com/beyond10x/ess/releases/download/0.99.1/ess-0.99.1-x86_64-unknown-linux-gnu.tar.gz' "$R5/.calls" && grep -qx 'fetch https://registry.npmjs.org/undici-types/-/undici-types-8.3.0.tgz' "$R5/.calls" && ok "apply fetches the three trees that differ and no other" || bad "home fetches: $(grep '^fetch' "$R5/.calls")"
grep -q '^changed: home:ess@0.99.1: ess 0.99.1 installed in /var/lib/builder/.local/share/ess-versions/0.99.1$' "$W/out" && ok "apply names the installed ess" || bad "home apply: $(grep home "$W/out" "$W/err")"
run "$R5" check
[ $? -eq 0 ] && ok "the home trees have 0 drift after apply" || bad "home after apply: $(cat "$W/out")"
rm -rf "$HP/ess-versions/0.99.4"
: >"$R5/.state-tool-fetch-fails"
run "$R5" apply
st=$?
rm -f "$R5/.state-tool-fetch-fails"
[ $st -eq 1 ] && grep -qx 'failed: home:ess@0.99.4: download or checksum of ess 0.99.4' "$W/err" && ok "a failed ess download: apply exits 1 and names it" || bad "failed home download: exit $st $(grep home "$W/err")"
[ ! -e "$HP/ess-versions/0.99.4" ] && ok "a failed download leaves no tree" || bad "a tree of a failed download: $(ls "$HP/ess-versions/0.99.4")"
run "$R5" apply
rm -rf "$R5/var/lib/builder/.cache/ms-playwright/chromium_headless_shell-1234"
printf '#!/bin/sh\necho "Google Chrome for Testing 1.0.0.0"\n' >"$R5/var/lib/builder/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell"
run "$R5" check
grep -q "^drift: home:chrome-headless-shell@151.0.7922.34: .*chromium_headless_shell-1234 is 'missing', want 151.0.7922.34" "$W/out" && grep -q "^drift: home:chrome-headless-shell@158.0.8127.18: .*chromium_headless_shell-1254 is '1.0.0.0', want 158.0.8127.18" "$W/out" && ok "check names a missing browser and a browser of another version" || bad "browser drift: $(grep home "$W/out")"
run "$R5" apply
run "$R5" check
[ $? -eq 0 ] && ok "the browsers have 0 drift after apply" || bad "browsers after apply: $(cat "$W/out")"
GJ=$R5/var/lib/builder/.sdkman/candidates/groovy/2.4.10/embeddable/groovy-all-2.4.10.jar
[ -f "$GJ" ] && ok "groovy 2.4.10 (stub download) is at ~/.sdkman/candidates/groovy/2.4.10 with embeddable/groovy-all-2.4.10.jar" || bad "groovy jar: $(ls -R "$R5/var/lib/builder/.sdkman" 2>&1 | head -5)"
rm -f "$GJ"
run "$R5" check
grep -q "^drift: home:groovy@2.4.10: .*groovy/2.4.10 is 'missing', want 2.4.10" "$W/out" && ok "check names a groovy tree without its jar" || bad "groovy drift: $(grep home "$W/out")"
rm -f "$GJ"
echo jar >"$W/other.jar"
ln -s "$W/other.jar" "$GJ"
run "$R5" check
grep -q "^drift: home:groovy@2.4.10: .*groovy/2.4.10 is 'missing', want 2.4.10" "$W/out" && ok "a jar that is a symbolic link to a regular file is drift" || bad "groovy link: $(grep home "$W/out")"
run "$R5" apply
run "$R5" check
[ $? -eq 0 ] && [ -f "$GJ" ] && [ ! -L "$GJ" ] && ok "groovy has 0 drift after apply" || bad "groovy after apply: $(cat "$W/out")"
# sdkman's `current` link: ~/.sdkman/candidates/groovy/current names the version of the groovy line
# of HOME_PINS, relative, as sdkman makes it. A job that carries GROOVY_HOME of the workstation
# (that path) finds the tree.
GC=/var/lib/builder/.sdkman/candidates/groovy/current
[ "$(readlink "$R5$GC")" = 2.4.10 ] && ok "apply links $GC to 2.4.10" || bad "groovy current after apply: '$(readlink "$R5$GC")'"
rm -f "$R5$GC"
run "$R5" check
st=$?
[ $st -eq 1 ] && grep -qx "drift: home:groovy-current: $GC is 'missing', want a link to 2.4.10" "$W/out" && ok "check names a missing groovy current link" || bad "groovy current missing: exit $st $(grep groovy "$W/out")"
run "$R5" apply
st=$?
[ $st -eq 0 ] && grep -qx "changed: home:groovy-current: $GC links to 2.4.10" "$W/out" && [ "$(readlink "$R5$GC")" = 2.4.10 ] && ok "apply creates the groovy current link" || bad "groovy current apply: exit $st $(grep groovy "$W/out" "$W/err") '$(readlink "$R5$GC")'"
ln -sfn 3.0.13 "$R5$GC"
run "$R5" check
st=$?
[ $st -eq 1 ] && grep -qx "drift: home:groovy-current: $GC is '3.0.13', want a link to 2.4.10" "$W/out" && ok "a current link to another version is drift" || bad "groovy current other: exit $st $(grep groovy "$W/out")"
run "$R5" apply
run "$R5" check
[ $? -eq 0 ] && [ "$(readlink "$R5$GC")" = 2.4.10 ] && ok "apply sets the current link back to 2.4.10" || bad "groovy current back: $(cat "$W/out") '$(readlink "$R5$GC")'"
rm -f "$R5$GC"
mkdir "$R5$GC"
run "$R5" check
grep -qx "drift: home:groovy-current: $GC is a directory, want a link to 2.4.10" "$W/out" && ok "a directory at the current path is drift" || bad "groovy current directory: $(grep groovy "$W/out")"
run "$R5" apply
st=$?
[ $st -eq 1 ] && grep -qx "failed: home:groovy-current: $GC is no symbolic link: not replaced" "$W/err" && [ -d "$R5$GC" ] && [ ! -L "$R5$GC" ] && ok "apply leaves a directory at the current path and says so" || bad "groovy current over a directory: exit $st $(grep groovy "$W/out" "$W/err")"
rmdir "$R5$GC"
run "$R5" apply

# 5i. GO_ROOT: the directory the go archive goes to, for a node whose `go env GOROOT` must answer
# the path the workstation's go answers (a consumer's output carries that path). Empty, go stays
# in /usr/local/go-<version>. With it the archive is a real directory at that path, with the mark
# .slot-archive in it (`go <version> <sha256>`), and `go env GOROOT` as the build user is
# compared. Under NODE_TEST=1 the stand-in for go answers the directory it stands in, and the
# file goroot under .state answers in its place; pkg-owner/<path, _ for /> names the package that
# owns a path.
conf
echo 0 >"$R5/.state/jobs"
run "$R5" apply
run "$R5" check
[ $? -eq 0 ] && [ "$(readlink "$R5/usr/local/bin/go")" = /usr/local/go-1.27.1/bin/go ] && [ ! -e "$R5/usr/local/go-1.27.1/.slot-archive" ] && ! grep -q '^pending' "$W/out" && ok "without GO_ROOT go stays in /usr/local/go-1.27.1, as before" || bad "without GO_ROOT: $(cat "$W/out") $(readlink "$R5/usr/local/bin/go")"
gosum=63d339f0da5ab53635a56f2490a7984dfe12dfcff22ad749f63edaf590168445
echo 'GO_ROOT=/usr/lib/go' >>"$W/node.conf"
run "$R5" check
st=$?
[ $st -eq 1 ] && grep -qx "drift: archive:go: /usr/local/bin/go is '/usr/local/go-1.27.1/bin/go', want a link to /usr/lib/go/bin/go" "$W/out" && grep -qx "drift: archive:go: /usr/local/bin/gofmt is '/usr/local/go-1.27.1/bin/gofmt', want a link to /usr/lib/go/bin/gofmt" "$W/out" && grep -qx "drift: archive:go: /usr/lib/go/bin/go says 'missing', want go1.27.1" "$W/out" && ! grep -q '^pending' "$W/out" && ok "GO_ROOT=/usr/lib/go: check names the links and the go that is not there yet" || bad "GO_ROOT check: exit $st $(grep 'archive\|pending' "$W/out")"
# The move: the new directory first, then the links; the old one goes when no job runs.
echo 3 >"$R5/.state/jobs"
: >"$R5/.calls"
run "$R5" apply
st=$?
[ $st -eq 0 ] && grep -qx 'changed: archive:go: go 1.27.1 installed in /usr/lib/go, linked in /usr/local/bin' "$W/out" && [ "$(readlink "$R5/usr/local/bin/go")" = /usr/lib/go/bin/go ] && [ "$(readlink "$R5/usr/local/bin/gofmt")" = /usr/lib/go/bin/gofmt ] && ok "apply installs go in GO_ROOT and links /usr/local/bin/go and gofmt into it" || bad "GO_ROOT apply: exit $st $(grep archive "$W/out" "$W/err") $(readlink "$R5/usr/local/bin/go")"
[ -d "$R5/usr/lib/go" ] && [ ! -L "$R5/usr/lib/go" ] && [ "$(cat "$R5/usr/lib/go/.slot-archive")" = "go 1.27.1 $gosum" ] && [ -z "$(find "$R5/usr/lib/go" -perm /022 ! -type l)" ] && ok "GO_ROOT is a real directory with the mark .slot-archive: go, the version, the SHA-256 of the archive" || bad "GO_ROOT directory: $(ls -la "$R5/usr/lib/go" 2>&1 | head -5) $(cat "$R5/usr/lib/go/.slot-archive" 2>&1)"
[ "$("$R5/usr/lib/go/bin/go" env GOROOT)" = "$R5/usr/lib/go" ] && [ "$("$R5/usr/lib/go/bin/go" version)" = 'go version go1.27.1 linux/amd64' ] && ok "the stand-in for go answers the directory it stands in for env GOROOT" || bad "go stand-in: $("$R5/usr/lib/go/bin/go" env GOROOT 2>&1)"
grep -qx 'pending: archive:go: /usr/local/go-1.27.1 is removed when no job runs' "$W/err" && [ -x "$R5/usr/local/go-1.27.1/bin/go" ] && ok "with 3 jobs running the old /usr/local/go-1.27.1 stays, and a pending line says until when" || bad "old directory with jobs: $(cat "$W/err") $(ls "$R5/usr/local" | tr '\n' ' ')"
run "$R5" check
st=$?
[ $st -eq 0 ] && grep -qx 'pending: archive:go: /usr/local/go-1.27.1 is removed when no job runs' "$W/out" && grep -qx '0 drift' "$W/out" && ok "check names the old directory as pending, not as drift: 0 drift, exit 0" || bad "check with the old directory: exit $st $(cat "$W/out")"
: >"$R5/.calls"
run "$R5" apply
st=$?
[ $st -eq 0 ] && grep -qx '0 changed' "$W/out" && grep -qx 'pending: archive:go: /usr/local/go-1.27.1 is removed when no job runs' "$W/err" && [ -d "$R5/usr/local/go-1.27.1" ] && ! grep -q '^fetch' "$R5/.calls" && ok "a second apply with jobs: pending again, nothing fetched, nothing changed" || bad "second apply with jobs: exit $st $(cat "$W/out" "$W/err") $(cat "$R5/.calls")"
: >"$R5/.state/jobs"
run "$R5" apply
grep -qx 'pending: archive:go: /usr/local/go-1.27.1 is removed when no job runs' "$W/err" && [ -d "$R5/usr/local/go-1.27.1" ] && ok "a pool that does not say how many jobs run counts as one with jobs: the old directory stays" || bad "unknown job count: $(cat "$W/out" "$W/err")"
echo 0 >"$R5/.state/jobs"
run "$R5" apply
st=$?
[ $st -eq 0 ] && grep -qx 'changed: archive:go: /usr/local/go-1.27.1 removed: go is in /usr/lib/go' "$W/out" && [ ! -e "$R5/usr/local/go-1.27.1" ] && [ -x "$R5/usr/lib/go/bin/go" ] && ok "with no job running apply removes the old /usr/local/go-1.27.1" || bad "old directory without jobs: exit $st $(cat "$W/out" "$W/err")"
run "$R5" check
[ $? -eq 0 ] && ! grep -q '^pending' "$W/out" && ok "after the move: 0 drift and no pending line" || bad "after the move: $(cat "$W/out")"
# An old directory that a link of /usr/local/bin still names is not removed.
mkdir -p "$R5/usr/local/go-1.27.1/bin"
ln -sfn /usr/local/go-1.27.1/bin/gofmt "$R5/usr/local/bin/gofmt"
run "$R5" check
grep -qx "drift: archive:go: /usr/local/bin/gofmt is '/usr/local/go-1.27.1/bin/gofmt', want a link to /usr/lib/go/bin/gofmt" "$W/out" && ! grep -q '^pending' "$W/out" && ok "an old directory that /usr/local/bin still links to is drift of the link, not a pending removal" || bad "old directory still linked: $(grep 'archive\|pending' "$W/out")"
run "$R5" apply
run "$R5" check
[ $? -eq 0 ] && [ ! -e "$R5/usr/local/go-1.27.1" ] && [ "$(readlink "$R5/usr/local/bin/gofmt")" = /usr/lib/go/bin/gofmt ] && ok "apply sets the link and then removes the old directory" || bad "old directory after the link: $(cat "$W/out") $(ls "$R5/usr/local" | tr '\n' ' ')"
# What go answers is compared: another answer than GO_ROOT is drift, and apply does not call a go
# that still answers it installed.
echo /usr/local/go-1.27.1 >"$R5/.state/goroot"
run "$R5" check
st=$?
[ $st -eq 1 ] && grep -qx "drift: archive:go: go env GOROOT of /usr/lib/go/bin/go answers '/usr/local/go-1.27.1', want /usr/lib/go" "$W/out" && ok "check compares what go env GOROOT answers with GO_ROOT and names both" || bad "GOROOT drift: exit $st $(grep archive "$W/out")"
run "$R5" apply
st=$?
[ $st -eq 1 ] && grep -qx "failed: archive:go: go env GOROOT of /usr/lib/go/bin/go answers '/usr/local/go-1.27.1' after the install, want /usr/lib/go" "$W/err" && ! grep -q '^changed: archive:go' "$W/out" && ok "a go that answers another GOROOT after the install: failed line, exit 1, not reported as changed" || bad "GOROOT after the install: exit $st $(grep archive "$W/out" "$W/err")"
: >"$R5/.state/goroot"
run "$R5" check
grep -qx "drift: archive:go: go env GOROOT of /usr/lib/go/bin/go answers 'nothing', want /usr/lib/go" "$W/out" && ok "a go that answers nothing for env GOROOT is drift" || bad "no GOROOT answer: $(grep archive "$W/out")"
rm -f "$R5/.state/goroot"
run "$R5" check
[ $? -eq 0 ] || bad "GOROOT after the stand-in answers again: $(cat "$W/out")"
# A release change with GO_ROOT set replaces the directory whole, as it does without GO_ROOT.
printf '#!/bin/sh\necho "go version go1.26.0 linux/amd64"\n' >"$R5/usr/lib/go/bin/go"
echo 'go 1.26.0 0000' >"$R5/usr/lib/go/.slot-archive"
: >"$R5/usr/lib/go/file-of-the-old-release"
run "$R5" check
grep -qx "drift: archive:go: /usr/lib/go/bin/go says 'go version go1.26.0 linux/amd64', want go1.27.1" "$W/out" && ok "check names another release in GO_ROOT" || bad "release in GO_ROOT: $(grep archive "$W/out")"
: >"$R5/.calls"
run "$R5" apply
st=$?
[ $st -eq 0 ] && grep -qx 'fetch https://go.dev/dl/go1.27.1.linux-amd64.tar.gz' "$R5/.calls" && [ ! -e "$R5/usr/lib/go/file-of-the-old-release" ] && [ "$(cat "$R5/usr/lib/go/.slot-archive")" = "go 1.27.1 $gosum" ] && [ ! -e "$R5/usr/lib/go.slot-new" ] && ok "apply replaces the directory of the old release whole and writes the mark of the new one" || bad "release change in GO_ROOT: exit $st $(grep archive "$W/out" "$W/err") $(ls -a "$R5/usr/lib/go")"
run "$R5" check
[ $? -eq 0 ] || bad "after the release change in GO_ROOT: $(cat "$W/out")"
# A directory that is there and does not carry the mark is not provision.sh's: it stays as it is.
conf
echo 'GO_ROOT=/opt/go' >>"$W/node.conf"
mkdir -p "$R5/opt/go/bin"
echo theirs >"$R5/opt/go/README"
run "$R5" check
st=$?
[ $st -eq 1 ] && grep -qx "drift: archive:go: GO_ROOT /opt/go exists without the mark of provision.sh (/opt/go/.slot-archive), owner $me" "$W/out" && [ "$(grep -c '^drift: archive:go' "$W/out")" -eq 1 ] && ok "a GO_ROOT that exists without the mark: one drift line that names its owner" || bad "foreign GO_ROOT, check: exit $st $(grep archive "$W/out")"
: >"$R5/.calls"
run "$R5" apply
st=$?
[ $st -eq 1 ] && grep -qx "failed: archive:go: GO_ROOT /opt/go exists without the mark of provision.sh (/opt/go/.slot-archive), owner $me: nothing was changed" "$W/err" && [ "$(grep -c 'archive:go' "$W/err")" -eq 1 ] && [ "$(cat "$R5/opt/go/README")" = theirs ] && [ "$(LC_ALL=C ls -A "$R5/opt/go" | tr '\n' ' ')" = 'README bin ' ] && [ "$(readlink "$R5/usr/local/bin/go")" = /usr/lib/go/bin/go ] && ! grep -q 'go.dev' "$R5/.calls" && ok "apply does not write into it: one failed line with the owner, exit 1, the directory and the links as they were, nothing fetched" || bad "foreign GO_ROOT, apply: exit $st $(grep archive "$W/out" "$W/err") $(ls -A "$R5/opt/go") $(readlink "$R5/usr/local/bin/go")"
rm -rf "$R5/opt/go"
ln -s /nowhere "$R5/opt/go"
run "$R5" apply
st=$?
[ $st -eq 1 ] && grep -qx "failed: archive:go: GO_ROOT /opt/go is a symbolic link, owner $me: nothing was changed" "$W/err" && [ "$(readlink "$R5/opt/go")" = /nowhere ] && ok "a GO_ROOT that is a symbolic link is not replaced" || bad "GO_ROOT link: exit $st $(grep archive "$W/out" "$W/err")"
rm -f "$R5/opt/go"
# A mark that is not a file of its own (a link) is no mark.
mkdir -p "$R5/opt/go"
echo "go 1.27.1 $gosum" >"$W/mark"
ln -s "$W/mark" "$R5/opt/go/.slot-archive"
run "$R5" apply
st=$?
[ $st -eq 1 ] && grep -q '^failed: archive:go: GO_ROOT /opt/go exists without the mark of provision.sh' "$W/err" && ok "a mark that is a symbolic link does not count" || bad "mark link: exit $st $(grep archive "$W/out" "$W/err")"
rm -rf "$R5/opt/go"
# A path a package owns is the package's, also with the mark in it.
conf
echo 'GO_ROOT=/usr/lib/go' >>"$W/node.conf"
mkdir -p "$R5/.state/pkg-owner"
echo golang-1.22.5-1.amzn2023.x86_64 >"$R5/.state/pkg-owner/_usr_lib_go"
run "$R5" check
st=$?
[ $st -eq 1 ] && grep -qx 'drift: archive:go: GO_ROOT /usr/lib/go belongs to the package golang-1.22.5-1.amzn2023.x86_64' "$W/out" && ok "a GO_ROOT that a package owns: drift that names the package" || bad "package GO_ROOT, check: exit $st $(grep archive "$W/out")"
rm -f "$R5/usr/local/bin/go"
: >"$R5/.calls"
run "$R5" apply
st=$?
[ $st -eq 1 ] && grep -qx 'failed: archive:go: GO_ROOT /usr/lib/go belongs to the package golang-1.22.5-1.amzn2023.x86_64: nothing was changed' "$W/err" && [ ! -e "$R5/usr/local/bin/go" ] && ! grep -q 'go.dev' "$R5/.calls" && ok "apply does not write into a package's directory: failed line with the package, exit 1, no link set, nothing fetched" || bad "package GO_ROOT, apply: exit $st $(grep archive "$W/out" "$W/err")"
rm -rf "$R5/.state/pkg-owner"
run "$R5" apply
run "$R5" check
[ $? -eq 0 ] && [ "$(readlink "$R5/usr/local/bin/go")" = /usr/lib/go/bin/go ] && ok "without the package: apply sets the link again, 0 drift" || bad "after the package: $(cat "$W/out")"
# A path with a symbolic link in it is not written through.
conf
echo 'GO_ROOT=/opt/via/go' >>"$W/node.conf"
mkdir -p "$W/elsewhere" "$R5/opt"
ln -s "$W/elsewhere" "$R5/opt/via"
run "$R5" apply
st=$?
[ $st -eq 1 ] && grep -qx 'failed: archive:go: GO_ROOT /opt/via/go has a symbolic link in its path: not written' "$W/err" && [ -z "$(ls -A "$W/elsewhere")" ] && ok "a GO_ROOT with a symbolic link in its path is not written" || bad "GO_ROOT through a link: exit $st $(grep archive "$W/out" "$W/err") $(ls -A "$W/elsewhere")"
rm -f "$R5/opt/via"
# The real queries, each alone: go as the build user, and rpm for the owner.
gq=$(
  eval "$(sed -n '/^q_goroot() {$/,/^}$/p; /^q_pkg_owner() {$/,/^}$/p' "$P")"
  T= R=
  as_user() { echo "as_user $*"; }
  q_goroot /usr/lib/go/bin/go
  rpm() {
    echo "rpm $*" >"$W/rpm-args"
    echo golang-bin-1.22.5-1.amzn2023.x86_64
    echo golang-1.22.5-1.amzn2023.x86_64
  }
  q_pkg_owner /usr/lib/go
  rpm() {
    echo 'file /usr/lib/go is not owned by any package'
    return 1
  }
  q_pkg_owner /usr/lib/go
  echo end
)
[ "$gq" = "as_user /usr/lib/go/bin/go env GOROOT
golang-bin-1.22.5-1.amzn2023.x86_64
end" ] && [ "$(cat "$W/rpm-args")" = 'rpm -qf /usr/lib/go' ] && ok "without NODE_TEST: go env GOROOT runs as the build user, and rpm -qf names the owner (nothing for a path no package owns)" || bad "real GOROOT queries: $gq / $(cat "$W/rpm-args")"
# GO_ROOT in the config: an absolute path, and none that provisioning manages.
gread() { sh -c '. "$1/conf.sh"; conf_read "$2" BUILD_USER && echo "[$GO_ROOT]"' sh "$here" "$W/node.conf" >"$W/out" 2>"$W/err"; }
conf
gread
[ $? -eq 0 ] && [ "$(cat "$W/out")" = '[]' ] && ok "GO_ROOT is a config key, empty by default" || bad "GO_ROOT default: $(cat "$W/out" "$W/err")"
for good_g in /usr/lib/go /opt/go /usr/lib/golang /usr/local/go /opt/go-1.27/x86_64; do
  conf
  echo "GO_ROOT=$good_g" >>"$W/node.conf"
  gread
  [ $? -eq 0 ] && [ "$(cat "$W/out")" = "[$good_g]" ] && ok "GO_ROOT=$good_g is accepted" || bad "GO_ROOT=$good_g is refused ($(cat "$W/err"))"
done
for bad_g in usr/lib/go / /usr/lib/go/ /usr/lib/../go /usr/lib/.. '/usr/lib/my go' /usr//lib/go /usr/./lib/go '/usr/lib/go;x' '/usr/lib/$go' '/usr/lib/go*'; do
  conf
  echo "GO_ROOT=$bad_g" >>"$W/node.conf"
  gread
  [ $? -eq 2 ] && grep -q "GO_ROOT must be an absolute path .*, not '" "$W/err" && [ "$(grep -c . "$W/err")" -eq 1 ] && ok "GO_ROOT=$bad_g is refused" || bad "GO_ROOT=$bad_g is accepted ($(cat "$W/out" "$W/err"))"
done
for bad_g in /usr/local/bin /usr/local/bin/go /usr/local /usr /usr/bin /usr/local/libexec /usr/local/libexec/go /srv/slot /srv/slot/go /srv /var/lib/builder /var/lib/builder/go /var/lib /etc/go /run/go; do
  conf
  echo "GO_ROOT=$bad_g" >>"$W/node.conf"
  gread
  [ $? -eq 2 ] && grep -q "GO_ROOT '$bad_g' is a directory the provisioning manages, lies in one or holds one (" "$W/err" && [ "$(grep -c . "$W/err")" -eq 1 ] && ok "GO_ROOT=$bad_g is refused: provisioning manages it" || bad "GO_ROOT=$bad_g is accepted ($(cat "$W/out" "$W/err"))"
done
# The directories of the other pinned archives are known to provision.sh, which refuses them
# when it reads the config.
for bad_g in /usr/local/node-v22.23.3 /usr/local/node-v22.23.3/go /usr/local/go-1.27.1 /usr/local/musl-1.2.6 /usr/local/musl-1.2.6/go /usr/local/make-4.4.1; do
  conf
  echo "GO_ROOT=$bad_g" >>"$W/node.conf"
  : >"$R5/.calls"
  run "$R5" apply
  st=$?
  [ $st -eq 2 ] && grep -q "^node config: .*GO_ROOT '$bad_g' is the directory of a pinned archive, or lies in one (" "$W/err" && [ ! -s "$R5/.calls" ] && ok "GO_ROOT=$bad_g is refused: the directory of a pinned archive" || bad "GO_ROOT=$bad_g: exit $st $(head -2 "$W/err")"
done
sh "$P" bundle "$W/node.conf" check >"$W/out" 2>"$W/err"
[ $? -eq 2 ] && grep -q "GO_ROOT '/usr/local/make-4.4.1' is the directory of a pinned archive" "$W/err" && ok "a bundle of such a config is refused too" || bad "bundle with a GO_ROOT of a pin: $(head -2 "$W/err")"
conf
echo 'GO_ROOT=/usr/lib/go' >>"$W/node.conf"
run "$R5" apply
run "$R5" check
[ $? -eq 0 ] || bad "go in GO_ROOT again: $(cat "$W/out")"
# GO_ROOT taken out of the config: go goes to /usr/local/go-<version> again. The directory of
# the old GO_ROOT is no longer named by anything: it stays, for the owner to remove.
conf
run "$R5" apply
run "$R5" check
[ $? -eq 0 ] && [ "$(readlink "$R5/usr/local/bin/go")" = /usr/local/go-1.27.1/bin/go ] && [ -e "$R5/usr/lib/go/.slot-archive" ] && ok "GO_ROOT taken out of the config: go is in /usr/local/go-1.27.1 again, 0 drift; the directory of the old GO_ROOT stays" || bad "GO_ROOT taken out: $(cat "$W/out") $(readlink "$R5/usr/local/bin/go")"
rm -rf "$R5/usr/lib/go"

# 5f. Rust toolchains: check names what is missing, apply installs it, a failed download is a
# `failed` line and exit 1, a toolchain that is there is not installed again.
conf
R2=$W/r2
mkdir -p "$R2"
run "$R2" check
grep -qx 'drift: rustup: missing' "$W/out" && ok "check names a missing rustup" || bad "rustup drift: $(grep rust "$W/out")"
grep -qx 'drift: rust:1.99.1: missing' "$W/out" && grep -qx 'drift: rust:stable: missing' "$W/out" && ok "check names each missing toolchain" || bad "toolchain drift: $(grep rust "$W/out")"
: >"$R2/.state-tool-fetch-fails"
run "$R2" apply
st=$?
rm -f "$R2/.state-tool-fetch-fails"
[ $st -eq 1 ] && grep -qx 'failed: rustup: download or checksum of rustup-init 1.29.3' "$W/err" && ok "a failed rustup download: apply exits 1 and names it" || bad "failed rustup download: exit $st $(grep rust "$W/err")"
grep -q '^changed: rustup' "$W/out" && bad "a failed rustup install is reported as changed" || ok "a failed rustup install is not reported as changed"
: >"$R2/.calls"
run "$R2" apply
run "$R2" check
[ $? -eq 0 ] && ok "rust: after the download works, 0 drift" || bad "rust after the download: $(cat "$W/out")"
[ -x "$R2/srv/slot/rustup/toolchains/1.99.1-x86_64-unknown-linux-gnu/bin/rustc" ] && [ -x "$R2/srv/slot/rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc" ] && ok "toolchains are in the store's rustup home" || bad "toolchain dirs: $(ls "$R2/srv/slot/rustup/toolchains" 2>&1)"
grep -qx 'rustup default stable' "$R2/.calls" && grep -qx 'default_toolchain = "stable-x86_64-unknown-linux-gnu"' "$R2/srv/slot/rustup/settings.toml" && ok "apply makes stable the default toolchain" || bad "rust default: $(cat "$R2/.calls" "$R2/srv/slot/rustup/settings.toml" 2>&1 | grep -i 'default')"
sed 's/stable-x86_64/1.99.1-x86_64/' "$R2/srv/slot/rustup/settings.toml" >"$R2/settings.new" && mv "$R2/settings.new" "$R2/srv/slot/rustup/settings.toml"
run "$R2" check
grep -qx "drift: rust:default: the default toolchain is '1.99.1-x86_64-unknown-linux-gnu', want stable-x86_64-unknown-linux-gnu" "$W/out" && ok "check names a default toolchain other than stable" || bad "default drift: $(grep rust "$W/out")"
: >"$R2/.calls"
run "$R2" apply
grep -qx 'changed: rust:default: rustup default stable (stable-x86_64-unknown-linux-gnu)' "$W/out" && [ "$(grep -c '^rustup toolchain install' "$R2/.calls")" = 0 ] && ok "apply sets the default again and installs nothing" || bad "default apply: $(cat "$W/out" "$R2/.calls")"
run "$R2" check
[ $? -eq 0 ] && ok "rust default: 0 drift after apply" || bad "rust default after apply: $(cat "$W/out")"
q="'"
printf 'version = "12"\n  default_toolchain = %sstable-x86_64-unknown-linux-gnu%s # pinned\nprofile = "minimal"\n' "$q" "$q" >"$R2/srv/slot/rustup/settings.toml"
run "$R2" check
[ $? -eq 0 ] && ok "rust default: a single-quoted value with a comment and other keys is read" || bad "settings.toml variant: $(cat "$W/out")"
printf 'version = "12"\ndefault_toolchain = "1.99.1-x86_64-unknown-linux-gnu"\n' >"$R2/srv/slot/rustup/settings.toml"
: >"$R2/.state-default-fails"
run "$R2" apply
st=$?
rm -f "$R2/.state-default-fails"
[ $st -eq 1 ] && grep -qx 'failed: rust:default: rustup default stable' "$W/err" && ok "a failed rustup default: apply exits 1 and names it" || bad "failed rustup default: exit $st $(grep rust "$W/err")"
run "$R2" apply
RD=$W/rdefault
mkdir -p "$RD"
conf
grep -v '^RUST_TOOLCHAINS=' "$W/node.conf" >"$W/c2" && mv "$W/c2" "$W/node.conf"
printf 'RUST_TOOLCHAINS=1.99.1\n' >>"$W/node.conf"
run "$RD" apply
grep -q 'rust:default' "$W/out" "$W/err" && bad "no stable in RUST_TOOLCHAINS, yet rust:default: $(grep 'rust:default' "$W/out" "$W/err")" || ok "without stable in RUST_TOOLCHAINS the default is left to rustup"
grep -q '^rustup default' "$RD/.calls" && bad "rustup default ran without stable: $(grep '^rustup default' "$RD/.calls")" || ok "without stable no rustup default call"
conf
rm -rf "$R2/srv/slot/rustup/toolchains/1.99.1-x86_64-unknown-linux-gnu"
: >"$R2/.calls"
run "$R2" check
grep -qx 'drift: rust:1.99.1: missing' "$W/out" && ! grep -q 'rust:stable\|rustup' "$W/out" && ok "check names only the toolchain that is gone" || bad "one toolchain gone: $(cat "$W/out")"
: >"$R2/.state-tool-fetch-fails"
run "$R2" apply
st=$?
rm -f "$R2/.state-tool-fetch-fails"
[ $st -eq 1 ] && grep -qx 'failed: rust:1.99.1: rustup toolchain install 1.99.1' "$W/err" && ok "a failed toolchain download: apply exits 1 and names it" || bad "failed toolchain download: exit $st $(grep rust "$W/err")"
: >"$R2/.calls"
run "$R2" apply
[ "$(cat "$R2/.calls" | grep -c '^rustup toolchain install')" = 1 ] && grep -qx 'rustup toolchain install 1.99.1 --profile minimal -c clippy -c rustfmt' "$R2/.calls" && ! grep -q 'rustup-init' "$R2/.calls" && ok "apply installs the missing toolchain only" || bad "reinstall calls: $(cat "$R2/.calls")"
# Components and targets from the config; no toolchain configured: rust is not touched.
R3=$W/r3
mkdir -p "$R3"
conf
printf 'RUST_COMPONENTS=clippy\nRUST_TARGETS=wasm32-unknown-unknown x86_64-unknown-linux-musl\n' >>"$W/node.conf"
run "$R3" apply
grep -qx 'rustup toolchain install stable --profile minimal -c clippy -t wasm32-unknown-unknown -t x86_64-unknown-linux-musl' "$R3/.calls" && ok "RUST_COMPONENTS and RUST_TARGETS reach rustup" || bad "components and targets: $(grep rustup "$R3/.calls")"
# A target after `<toolchain>:` goes to that toolchain only; a bare one goes to every toolchain.
R3b=$W/r3b
mkdir -p "$R3b"
conf
printf 'RUST_COMPONENTS=clippy 1.99.1:rust-src\nRUST_TARGETS=1.99.1:x86_64-unknown-linux-musl,wasm32-unknown-unknown wasm32-wasip1\n' >>"$W/node.conf"
run "$R3b" apply
grep -qx 'rustup toolchain install 1.99.1 --profile minimal -c clippy -c rust-src -t x86_64-unknown-linux-musl -t wasm32-unknown-unknown -t wasm32-wasip1' "$R3b/.calls" && grep -qx 'rustup toolchain install stable --profile minimal -c clippy -t wasm32-wasip1' "$R3b/.calls" && ok "RUST_TARGETS and RUST_COMPONENTS: <toolchain>:<names> reaches that toolchain, a bare name every one" || bad "per-toolchain targets: $(grep rustup "$R3b/.calls")"
run "$R3b" check
[ $? -eq 0 ] && ! grep -q 'target remove\|component remove' "$R3b/.calls" && ok "per-toolchain targets installed: 0 drift, and check removes nothing" || bad "per-toolchain targets, check: $(cat "$W/out") $(grep 'remove' "$R3b/.calls")"
# A target no entry names for its toolchain is surplus: check names it, apply removes it.
echo x86_64-unknown-linux-musl >>"$R3b/srv/slot/rustup/toolchains/stable-x86_64-unknown-linux-gnu/.targets"
run "$R3b" check
grep -qx 'drift: rust:stable: target x86_64-unknown-linux-musl is not in RUST_TARGETS' "$W/out" && ! grep -q 'rust:1.99.1' "$W/out" && ok "check names a surplus target of the toolchain that has it" || bad "surplus target, check: $(cat "$W/out")"
run "$R3b" apply
grep -qx 'rustup target remove --toolchain stable x86_64-unknown-linux-musl' "$R3b/.calls" && ! grep -q 'rustup target remove --toolchain 1.99.1' "$R3b/.calls" && ok "apply removes the surplus target and no other" || bad "surplus target, apply: $(grep 'target remove' "$R3b/.calls")"
run "$R3b" check
[ $? -eq 0 ] && ok "after the removal: 0 drift" || bad "after the surplus removal: $(cat "$W/out")"
# The same for the four components the manifest line carries; other components are not judged.
printf 'llvm-tools-x86_64-unknown-linux-gnu\nrust-docs-x86_64-unknown-linux-gnu\n' >>"$R3b/srv/slot/rustup/toolchains/stable-x86_64-unknown-linux-gnu/.components"
run "$R3b" check
grep -qx 'drift: rust:stable: component llvm-tools is not in RUST_COMPONENTS' "$W/out" && ! grep -q 'rust-docs' "$W/out" && ok "check names a surplus component of the manifest line, not rust-docs" || bad "surplus component, check: $(cat "$W/out")"
run "$R3b" apply
grep -qx 'rustup component remove --toolchain stable llvm-tools' "$R3b/.calls" && ok "apply removes the surplus component" || bad "surplus component, apply: $(grep 'component remove' "$R3b/.calls")"
run "$R3b" check
[ $? -eq 0 ] && ok "after the component removal: 0 drift" || bad "after the component removal: $(cat "$W/out")"
R4=$W/r4
mkdir -p "$R4"
conf
grep -v '^RUST_TOOLCHAINS=' "$W/node.conf" >"$W/c2" && mv "$W/c2" "$W/node.conf"
run "$R4" check
grep -q '^drift: rust' "$W/out" && bad "no RUST_TOOLCHAINS, yet rust drifts: $(grep '^drift: rust' "$W/out")" || ok "without RUST_TOOLCHAINS check says nothing about rust"
conf

# 5g. toolchains.sh: one line per rust toolchain; go, node and the C tools by the version they
# print (the machine it runs on), no digest of /usr or /etc.
TC=$W/tc
mkdir -p "$TC/usr/bin" "$TC/usr/lib/go/bin" "$TC/rh/toolchains/1.99.1-x86_64-unknown-linux-gnu/bin" "$TC/rh/toolchains/1.99.1-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib" "$TC/rh/toolchains/1.99.1-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-musl/lib"
: >"$TC/rh/toolchains/1.99.1-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib/libstd.rlib"
: >"$TC/rh/toolchains/1.99.1-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-musl/lib/libstd.rlib"
mkdir -p "$TC/rh/toolchains/1.99.1-x86_64-unknown-linux-gnu/lib/rustlib/wasm32-unknown-unknown/lib/self-contained"
printf 'rustc-x86_64-unknown-linux-gnu\nclippy-preview-x86_64-unknown-linux-gnu\nrustfmt-preview-x86_64-unknown-linux-gnu\nrust-docs-x86_64-unknown-linux-gnu\nrust-std-x86_64-unknown-linux-musl\nrust-src\nllvm-tools-preview-x86_64-unknown-linux-gnu\n' >"$TC/rh/toolchains/1.99.1-x86_64-unknown-linux-gnu/lib/rustlib/components"
printf '#!/bin/sh\necho "commit-hash: abc123"\n' >"$TC/rh/toolchains/1.99.1-x86_64-unknown-linux-gnu/bin/rustc"
printf '#!/bin/sh\necho "go version go1.24.0 linux/amd64"\n' >"$TC/usr/lib/go/bin/go"
printf '#!/bin/sh\necho v22.1.0\n' >"$TC/usr/bin/node"
printf '#!/bin/sh\necho "gcc (stub) 13.2.0"\necho second line\n' >"$TC/usr/bin/gcc"
chmod 755 "$TC/rh/toolchains/1.99.1-x86_64-unknown-linux-gnu/bin/rustc" "$TC/usr/lib/go/bin/go" "$TC/usr/bin/node" "$TC/usr/bin/gcc"
sh "$here/toolchains.sh" "$TC" "$TC/rh" >"$W/out" 2>"$W/err"
grep -qx 'rust 1.99.1-x86_64-unknown-linux-gnu abc123 clippy,llvm-tools,rust-src,rustfmt,x86_64-unknown-linux-gnu,x86_64-unknown-linux-musl' "$W/out" && ok "toolchains.sh: a rust line with name, commit, targets and clippy/llvm-tools/rust-src/rustfmt, no rust-docs" || bad "rust line: $(cat "$W/out" "$W/err")"
grep -qx 'go go1.24.0' "$W/out" && ok "toolchains.sh: go prints its bare version" || bad "go line: $(grep '^go' "$W/out")"
# The workstation's Arch build prints `go1.27.1-X:nodwarf5`; the node's archive build `go1.27.1`.
# Both give the same line. A node has go and node in /usr/local/bin (the archive pins).
mkdir -p "$TC/p2/usr/local/bin" "$TC/p2/usr/lib/go/bin" "$TC/p2/usr/bin"
printf '#!/bin/sh\necho "go version go1.27.1-X:nodwarf5 linux/amd64"\n' >"$TC/p2/usr/lib/go/bin/go"
printf '#!/bin/sh\necho "go version go1.27.1 linux/amd64"\n' >"$TC/p2/usr/local/bin/go"
printf '#!/bin/sh\necho v22.23.3\n' >"$TC/p2/usr/local/bin/node"
printf '#!/bin/sh\necho v18.0.0\n' >"$TC/p2/usr/bin/node"
chmod 755 "$TC/p2/usr/lib/go/bin/go" "$TC/p2/usr/local/bin/go" "$TC/p2/usr/local/bin/node" "$TC/p2/usr/bin/node"
sh "$here/toolchains.sh" "$TC/p2" "$TC/rh" 2>/dev/null | grep '^go \|^node ' | tr '\n' ',' >"$W/out2"
[ "$(cat "$W/out2")" = "go go1.27.1,node v22.23.3," ] && ok "toolchains.sh: the archive build's go and node in /usr/local/bin first" || bad "usr/local/bin lines: $(cat "$W/out2")"
rm -f "$TC/p2/usr/local/bin/go"
sh "$here/toolchains.sh" "$TC/p2" "$TC/rh" 2>/dev/null | grep '^go ' >"$W/out2"
[ "$(cat "$W/out2")" = "go go1.27.1" ] && ok "toolchains.sh: go1.27.1-X:nodwarf5 prints as go1.27.1" || bad "nodwarf5 line: $(cat "$W/out2")"
# goroot: what `go env GOROOT` of that go answers, on a line of its own. The `go ` line stays as
# it is: a wrapper compares the lines that begin with `go ` and a space.
printf '#!/bin/sh\nif [ "$*" = "env GOROOT" ]; then echo /usr/lib/go; else echo "go version go1.27.1 linux/amd64"; fi\n' >"$TC/p2/usr/local/bin/go"
chmod 755 "$TC/p2/usr/local/bin/go"
sh "$here/toolchains.sh" "$TC/p2" "$TC/rh" >"$W/out2" 2>/dev/null
[ "$(grep '^go' "$W/out2" | tr '\n' ',')" = 'go go1.27.1,goroot /usr/lib/go,' ] && ok "toolchains.sh: a goroot line with what go env GOROOT answers, after the go line" || bad "goroot line: $(grep '^go' "$W/out2" | tr '\n' ',')"
[ "$(grep -c '^go ' "$W/out2")" -eq 1 ] && [ "$(grep '^go ' "$W/out2")" = 'go go1.27.1' ] && ok "toolchains.sh: the goroot line does not begin with 'go ' and a space, and the go line is as before" || bad "go lines: $(grep '^go ' "$W/out2" | tr '\n' ',')"
printf '#!/bin/sh\nif [ "$*" = "env GOROOT" ]; then exit 1; else echo "go version go1.27.1 linux/amd64"; fi\n' >"$TC/p2/usr/local/bin/go"
sh "$here/toolchains.sh" "$TC/p2" "$TC/rh" 2>/dev/null | grep '^go' | tr '\n' ',' >"$W/out2"
[ "$(cat "$W/out2")" = 'go go1.27.1,' ] && ok "toolchains.sh: a go that names no GOROOT gets no goroot line" || bad "goroot of a go without an answer: $(cat "$W/out2")"
rm -f "$TC/p2/usr/local/bin/go"
! grep -q '^goroot' "$W/out" && ok "toolchains.sh: an answer that is no absolute path is no goroot line" || bad "goroot from a version line: $(grep '^goroot' "$W/out")"
mkdir -p "$TC/nogo/usr/bin"
sh "$here/toolchains.sh" "$TC/nogo" "$TC/rh" 2>/dev/null | grep -c '^go' >"$W/out2"
[ "$(cat "$W/out2")" = 0 ] && ok "toolchains.sh: without go neither a go line nor a goroot line" || bad "lines without go: $(cat "$W/out2")"
printf '#!/bin/sh\necho "GNU Make 4.4.1"\n' >"$TC/p2/usr/local/bin/make"
chmod 755 "$TC/p2/usr/local/bin/make"
sh "$here/toolchains.sh" "$TC/p2" "$TC/rh" 2>/dev/null | grep '^tool make ' >"$W/out2"
[ "$(cat "$W/out2")" = "tool make 4.4.1" ] && ok "toolchains.sh: a tool line for make" || bad "make tool line: $(cat "$W/out2")"
# musl: `tool musl <release>`, read from the loader beside the specs file that the musl-gcc
# wrapper names: /usr/local/musl-<release>/lib on a node (the wrapper linked in /usr/local/bin),
# /usr/lib/musl/lib on the workstation (the wrapper in /usr/bin). The c line reads either wrapper.
mkdir -p "$TC/p2/usr/local/musl-1.2.6/lib" "$TC/pm/usr/bin" "$TC/pm/usr/lib/musl/lib"
printf '#!/bin/sh\n[ "$1" != --version ] || { echo "gcc (stub) 11.5.0"; exit 0; }\nexec "${REALGCC:-gcc}" "$@" -specs "/usr/local/musl-1.2.6/lib/musl-gcc.specs"\n' >"$TC/p2/usr/local/bin/musl-gcc"
printf '#!/bin/sh\n[ "$1" != --version ] || { echo "gcc (stub) 16.2.1"; exit 0; }\nexec "${REALGCC:-gcc}" "$@" -specs "/usr/lib/musl/lib/musl-gcc.specs"\n' >"$TC/pm/usr/bin/musl-gcc"
for d in "$TC/p2/usr/local/musl-1.2.6/lib" "$TC/pm/usr/lib/musl/lib"; do
  printf '#!/bin/sh\necho "musl libc (x86_64)" >&2\necho "Version 1.2.6" >&2\necho "Dynamic Program Loader" >&2\nexit 1\n' >"$d/libc.so"
  chmod 755 "$d/libc.so"
done
chmod 755 "$TC/p2/usr/local/bin/musl-gcc" "$TC/pm/usr/bin/musl-gcc"
sh "$here/toolchains.sh" "$TC/p2" "$TC/rh" 2>/dev/null | grep '^tool musl\|^c musl-gcc' | tr '\n' ',' >"$W/out2"
[ "$(cat "$W/out2")" = "tool musl 1.2.6,c musl-gcc gcc (stub) 11.5.0," ] && ok "toolchains.sh: a node's musl-gcc in /usr/local/bin gives a tool line with the release and a c line" || bad "musl lines of a node: $(cat "$W/out2")"
sh "$here/toolchains.sh" "$TC/pm" "$TC/rh" 2>/dev/null | grep '^tool musl\|^c musl-gcc' | tr '\n' ',' >"$W/out2"
[ "$(cat "$W/out2")" = "tool musl 1.2.6,c musl-gcc gcc (stub) 16.2.1," ] && ok "toolchains.sh: a workstation's musl-gcc in /usr/bin gives the same tool line" || bad "musl lines of a workstation: $(cat "$W/out2")"
rm -f "$TC/pm/usr/lib/musl/lib/libc.so"
sh "$here/toolchains.sh" "$TC/pm" "$TC/rh" 2>/dev/null | grep '^tool musl' >"$W/out2"
[ "$(cat "$W/out2")" = "tool musl missing" ] && ok "toolchains.sh: a musl-gcc without a loader beside its specs prints missing as the release" || bad "musl without a loader: $(cat "$W/out2")"
sh "$here/toolchains.sh" "$TC/nogo" "$TC/rh" 2>/dev/null | grep '^tool musl\|^c musl-gcc' | tr '\n' ',' >"$W/out2"
[ "$(cat "$W/out2")" = "c musl-gcc missing," ] && ok "toolchains.sh: without musl-gcc no tool line for musl, and the c line says missing" || bad "musl lines without musl-gcc: $(cat "$W/out2")"
grep -qx 'node v22.1.0' "$W/out" && ok "toolchains.sh: node prints its version" || bad "node line: $(grep '^node' "$W/out")"
grep -qx 'c gcc gcc (stub) 13.2.0' "$W/out" && ok "toolchains.sh: gcc prints the first line of its version" || bad "gcc line: $(grep '^c gcc' "$W/out")"
grep -qx 'c clang missing' "$W/out" && ok "toolchains.sh: a missing compiler is named" || bad "clang line: $(grep '^c clang' "$W/out")"
grep -q '^c usr \|^c etc \|^c mingw-sysroot' "$W/out" && bad "toolchains.sh still digests /usr, /etc or the mingw sysroot: $(grep '^c usr\|^c etc\|^c mingw' "$W/out")" || ok "toolchains.sh: no digest of /usr, /etc or the mingw sysroot"
# tsc, prettier and esbuild: a `tool` line each, from /usr/local/bin first (the node), then
# /usr/bin, then the user's ~/.local/bin (the workstation); the version is the last word.
mkdir -p "$TC/p3/usr/local/bin" "$TC/p3/usr/bin" "$TC/home"
printf '#!/bin/sh\necho "Version 5.0.0"\n' >"$TC/p3/usr/bin/tsc"
printf '#!/bin/sh\necho "Version 6.0.3"\n' >"$TC/p3/usr/local/bin/tsc"
printf '#!/bin/sh\necho 3.8.1\n' >"$TC/p3/usr/bin/prettier"
chmod 755 "$TC/p3/usr/bin/tsc" "$TC/p3/usr/local/bin/tsc" "$TC/p3/usr/bin/prettier"
HOME=$TC/home sh "$here/toolchains.sh" "$TC/p3" "$TC/rh" 2>/dev/null | grep '^tool ' | tr '\n' ',' >"$W/out2"
[ "$(cat "$W/out2")" = "tool tsc 6.0.3,tool prettier 3.8.1," ] && ok "toolchains.sh: tool lines from /usr/local/bin first, a missing tool has no line" || bad "tool lines: $(cat "$W/out2")"
mkdir -p "$TC/home/.local/bin"
printf '#!/bin/sh\necho 0.28.2\n' >"$TC/home/.local/bin/esbuild"
chmod 755 "$TC/home/.local/bin/esbuild"
HOME=$TC/home sh "$here/toolchains.sh" "$TC/p3" "$TC/rh" 2>/dev/null | grep '^tool esbuild' >"$W/out2"
[ "$(cat "$W/out2")" = "tool esbuild 0.28.2" ] && ok "toolchains.sh: esbuild from the user's ~/.local/bin" || bad "esbuild line: $(cat "$W/out2")"
printf '#!/bin/sh\nexit 1\n' >"$TC/home/.local/bin/esbuild"
HOME=$TC/home sh "$here/toolchains.sh" "$TC/p3" "$TC/rh" 2>/dev/null | grep '^tool esbuild' >"$W/out2"
[ "$(cat "$W/out2")" = "tool esbuild missing" ] && ok "toolchains.sh: a tool that is there and prints no version is 'missing', not a line with a trailing space" || bad "broken tool line: '$(cat "$W/out2")'"
# pnpm and yarn are tools of the same loop (the workstation has yarn in /usr/bin). helm has no
# `--version`: its line comes from `helm version --template {{.Version}}`, and a helm on PATH is
# read only with the prefix "". The yarn here answers as yarn 1 does where a .yarnrc has a
# `yarn-path`: its own release only with YARN_IGNORE_PATH=1, else what that path prints.
printf '#!/bin/sh\necho 11.9.0\n' >"$TC/home/.local/bin/pnpm"
printf '#!/bin/sh\n[ "${YARN_IGNORE_PATH:-}" = 1 ] || { echo 9.9.9-from-yarn-path; exit 0; }\necho 1.22.22\n' >"$TC/p3/usr/bin/yarn"
printf '#!/bin/sh\n[ "$1" = version ] && [ "$2" = --template ] && [ "$3" = "{{.Version}}" ] && printf v3.8.1 && exit 0\necho "Error: unknown flag" >&2\nexit 1\n' >"$TC/p3/usr/local/bin/helm"
chmod 755 "$TC/home/.local/bin/pnpm" "$TC/p3/usr/bin/yarn" "$TC/p3/usr/local/bin/helm"
HOME=$TC/home sh "$here/toolchains.sh" "$TC/p3" "$TC/rh" 2>/dev/null | grep '^tool pnpm\|^tool yarn\|^tool helm' | tr '\n' ',' >"$W/out2"
[ "$(cat "$W/out2")" = "tool pnpm 11.9.0,tool yarn 1.22.22,tool helm v3.8.1," ] && ok "toolchains.sh: pnpm, yarn (read with YARN_IGNORE_PATH=1) and helm lines, helm through \`version --template\`" || bad "pnpm, yarn, helm lines: $(cat "$W/out2")"
mkdir -p "$TC/pathbin"
printf '#!/bin/sh\nprintf v9.9.9\n' >"$TC/pathbin/helm"
chmod 755 "$TC/pathbin/helm"
rm -f "$TC/p3/usr/local/bin/helm"
HOME=$TC/home PATH=$TC/pathbin:$PATH sh "$here/toolchains.sh" "$TC/p3" "$TC/rh" 2>/dev/null | grep '^tool helm\|^tool tsc' >"$W/out2"
[ "$(cat "$W/out2")" = "tool tsc 6.0.3" ] && ok "toolchains.sh: with a prefix the helm on PATH is not read" || bad "helm from PATH under a prefix: $(cat "$W/out2")"
HOME=$TC/home PATH=$TC/pathbin:$PATH sh "$here/toolchains.sh" "" "$TC/rh" 2>/dev/null | grep '^tool helm' >"$W/out2"
[ "$(cat "$W/out2")" = "tool helm v9.9.9" ] && ok "toolchains.sh: with the prefix \"\" the helm PATH names first is the one read" || bad "helm from PATH with the prefix \"\": '$(cat "$W/out2")'"
(cd "$TC/pathbin" && HOME=$TC/home PATH=.:$PATH sh "$here/toolchains.sh" "" "$TC/rh" 2>/dev/null | grep '^tool helm' >"$W/out2")
[ "$(cat "$W/out2")" = "tool helm v9.9.9" ] && ok "toolchains.sh: a helm that PATH names by a relative entry is read by its absolute path" || bad "helm from a relative PATH entry: '$(cat "$W/out2")'"
# The aep of runbook's driver pin: a line with the version of the directory and the first 16 hex
# digits of the file's SHA-256; the file is not run.
mkdir -p "$TC/home/.cache/sdc/aep/0.80.0/x86_64-unknown-linux-gnu" "$TC/home/.cache/sdc/aep/0.79.0/aarch64-apple-darwin"
printf 'not run\n' >"$TC/home/.cache/sdc/aep/0.80.0/x86_64-unknown-linux-gnu/aep"
printf 'other target\n' >"$TC/home/.cache/sdc/aep/0.79.0/aarch64-apple-darwin/aep"
SD=$(sha256sum "$TC/home/.cache/sdc/aep/0.80.0/x86_64-unknown-linux-gnu/aep" | cut -c1-16)
HOME=$TC/home sh "$here/toolchains.sh" "$TC/p3" "$TC/rh" 2>/dev/null | grep '^tool sdc-aep' >"$W/out2"
[ "$(cat "$W/out2")" = "tool sdc-aep-0.80.0 $SD" ] && ok "toolchains.sh: the pinned aep of the driver's cache is a tool line with its digest, other targets are left out" || bad "sdc-aep line: '$(cat "$W/out2")'"
rm -rf "$TC/home/.cache"
# Groovy under the sdkman path: a line per version directory with its groovy-all jar, a version
# directory that is a symbolic link included (sdkman's local install); the `current` link and a
# release without the jar print none.
mkdir -p "$TC/home/.sdkman/candidates/groovy/2.4.10/embeddable" "$TC/home/.sdkman/candidates/groovy/3.0.13/lib" "$TC/local-groovy/embeddable"
printf 'jar\n' >"$TC/home/.sdkman/candidates/groovy/2.4.10/embeddable/groovy-all-2.4.10.jar"
printf 'local jar\n' >"$TC/local-groovy/embeddable/groovy-all-2.4.9.jar"
ln -s "$TC/local-groovy" "$TC/home/.sdkman/candidates/groovy/2.4.9"
ln -s 2.4.10 "$TC/home/.sdkman/candidates/groovy/current"
GD=$(sha256sum "$TC/home/.sdkman/candidates/groovy/2.4.10/embeddable/groovy-all-2.4.10.jar" | cut -c1-16)
GL=$(sha256sum "$TC/local-groovy/embeddable/groovy-all-2.4.9.jar" | cut -c1-16)
HOME=$TC/home sh "$here/toolchains.sh" "$TC/p3" "$TC/rh" 2>/dev/null | grep '^tool groovy' >"$W/out2"
[ "$(cat "$W/out2")" = "tool groovy-all-2.4.10 $GD
tool groovy-all-2.4.9 $GL" ] && ok "toolchains.sh: the groovy-all jar of a version directory is a tool line with its digest, a linked version directory too; the current link and a release without the jar print none" || bad "groovy line: '$(cat "$W/out2")'"
rm -rf "$TC/home/.sdkman"
# ess, aep, yq and cargo-about: the user's cargo bin and ~/go/bin are searched too. The command runs
# in / and without ESS_TOOLCHAIN: the ess dispatcher prints the release a tree requires (an
# ess-inputs.yaml in the working directory, ESS_TOOLCHAIN in the environment), not its own.
mkdir -p "$TC/home/.cargo/bin" "$TC/home/go/bin" "$TC/tree"
printf '#!/bin/sh\n[ -e ess-inputs.yaml ] && { echo "ess 0.98.0"; exit 0; }\necho "ess ${ESS_TOOLCHAIN:-0.99.2}"\n' >"$TC/home/.cargo/bin/ess"
printf '#!/bin/sh\necho "aep 0.80.0"\n' >"$TC/home/.cargo/bin/aep"
printf '#!/bin/sh\necho "yq (https://github.com/mikefarah/yq/) version v4.30.8"\n' >"$TC/home/go/bin/yq"
chmod 755 "$TC/home/.cargo/bin/ess" "$TC/home/.cargo/bin/aep" "$TC/home/go/bin/yq"
: >"$TC/tree/ess-inputs.yaml"
(cd "$TC/tree" && ESS_TOOLCHAIN=0.97.4 HOME=$TC/home sh "$here/toolchains.sh" "$TC/p3" "$TC/rh" 2>/dev/null | grep '^tool ess\|^tool aep\|^tool yq\|^tool cargo-about' | tr '\n' ',' >"$W/out2")
[ "$(cat "$W/out2")" = "tool ess 0.99.2,tool aep 0.80.0,tool yq v4.30.8," ] && ok "toolchains.sh: ess, aep and yq lines from the user's cargo bin and ~/go/bin, ess from /, cargo-about missing has no line" || bad "ess, aep, yq lines: $(cat "$W/out2")"

# 5c. Closing the setup login ends the connections still logged in with the setup key; the
# build key's connections stay. The node's sshd log names each connection's pid and key.
ssh-keygen -q -t ed25519 -N '' -C setup -f "$W/setupkey" </dev/null
fp=$(ssh-keygen -lf "$W/setupkey.pub" | awk '{print $2}')
conf
echo "SETUP_KEY=$(cat "$W/setupkey.pub")" >>"$W/node.conf"
run "$R" apply
# journalctl -o short-unix lines. Older OpenSSH logs as sshd[<pid>]; from 9.8 the
# per-connection process is sshd-session. Pid 4646 logged a setup login, ended, and its number
# now belongs to another program; pid 4747's number was taken by a later connection.
# alive-pids: pid, process name, start time.
printf '%s\n' "1795600001.000000 node sshd[4242]: Accepted publickey for $me from 192.0.2.1 port 5 ssh2: ED25519 $fp" \
  "1795600002.000000 node sshd[4343]: Accepted publickey for $me from 192.0.2.1 port 6 ssh2: ED25519 SHA256:BuildKeyFingerprint" \
  "1795600003.000000 node sshd[4444]: Accepted publickey for $me from 192.0.2.1 port 7 ssh2: ED25519 $fp" \
  "1795600004.000000 node sshd-session[4545]: Accepted publickey for $me from 192.0.2.1 port 8 ssh2: ED25519 $fp" \
  "1795600005.000000 node sshd-session[4646]: Accepted publickey for $me from 192.0.2.1 port 9 ssh2: ED25519 $fp" \
  "1795600006.000000 node sshd-session[4747]: Accepted publickey for $me from 192.0.2.1 port 10 ssh2: ED25519 $fp" >"$R/.state/sshd-journal"
printf '4242 sshd 1795600001\n4343 sshd 1795600002\n4545 sshd-session 1795600004\n4646 bash 1795600005\n4747 sshd-session 1795609999\n' >"$R/.state/alive-pids"
NODE_TEST=1 NODE_ROOT=$R sh "$P" check "$W/node.conf" --open >"$W/out" 2>"$W/err"
grep -q 'setup key' "$W/out" && grep -q '^drift: keys: a login' "$W/out" && bad "while open, a setup login is drift: $(grep '^drift' "$W/out")" || ok "while the setup login is open, its connections are not drift"
run "$R" check
st=$?
[ $st -eq 1 ] && grep -qx 'drift: keys: a login with the setup key is still connected: pid 4242' "$W/out" && ok "closed: a connected setup login is drift" || bad "closed setup login: exit $st $(cat "$W/out")"
grep -qx 'drift: keys: a login with the setup key is still connected: pid 4545' "$W/out" && ok "an sshd-session login is drift" || bad "sshd-session login not found: $(cat "$W/out")"
grep -q 'pid 4343\|pid 4444\|pid 4646\|pid 4747' "$W/out" && bad "a build login, an ended login or a reused pid is drift: $(grep pid "$W/out")" || ok "build logins, ended logins and reused pids are not drift"
: >"$R/.calls"
run "$R" apply
grep -qx 'kill 4242' "$R/.calls" && ok "apply ends the setup login's connection" || bad "no kill: $(cat "$R/.calls")"
grep -qx 'kill 4545' "$R/.calls" && ok "apply ends the sshd-session connection" || bad "no kill of 4545: $(cat "$R/.calls")"
grep -q 'kill 4343\|kill 4646\|kill 4747' "$R/.calls" && bad "apply ended a build login or another program" || ok "apply leaves the build login and other programs"
run "$R" check
[ $? -eq 0 ] && ok "after the setup connection ended: 0 drift" || bad "after the kill: $(cat "$W/out")"
rm -f "$R/.state/sshd-journal" "$R/.state/alive-pids"

# 5d. With logind, each login is a session whose scope holds every process it started, also one
# whose parent became pid 1. Closing ends the setup login's whole session: session 7 is live
# (leader 5050, its user process 5051, a command 5052); session 8's leader 6060 has ended and
# one command (6070) still runs. Session 9 is the build key's; session 10's leader pid 6161
# logged a setup login long before the session began, so the number was reused.
# sessions/<id>: LEADER and REALTIME (microseconds) as in /run/systemd/sessions/<id>.
printf '%s\n' "1795600010.000000 node sshd-session[5050]: Accepted publickey for $me from 192.0.2.1 port 11 ssh2: ED25519 $fp" \
  "1795600020.000000 node sshd-session[6060]: Accepted publickey for $me from 192.0.2.1 port 12 ssh2: ED25519 $fp" \
  "1795600021.000000 node sshd-session[4343]: Accepted publickey for $me from 192.0.2.1 port 13 ssh2: ED25519 SHA256:BuildKeyFingerprint" \
  "1795600030.000000 node sshd-session[6161]: Accepted publickey for $me from 192.0.2.1 port 14 ssh2: ED25519 $fp" >"$R/.state/sshd-journal"
printf '5050 sshd-session 1795600010\n4343 sshd-session 1795600021\n6161 sshd-session 1795700000\n' >"$R/.state/alive-pids"
mkdir -p "$R/.state/sessions" "$R/.state/session-procs"
printf 'LEADER=5050\nREALTIME=1795600010200000\n' >"$R/.state/sessions/7"
printf 'LEADER=6060\nREALTIME=1795600020100000\n' >"$R/.state/sessions/8"
printf 'LEADER=4343\nREALTIME=1795600021100000\n' >"$R/.state/sessions/9"
printf 'LEADER=6161\nREALTIME=1795700000000000\n' >"$R/.state/sessions/10"
echo '5050 5051 5052' >"$R/.state/session-procs/7"
echo '6070' >"$R/.state/session-procs/8"
echo '4343 4344' >"$R/.state/session-procs/9"
echo '6161 6162' >"$R/.state/session-procs/10"
run "$R" check
st=$?
[ $st -eq 1 ] && grep -qx 'drift: keys: a login with the setup key is still running: session 7 (leader pid 5050)' "$W/out" && ok "a live setup session is drift" || bad "live session: exit $st $(cat "$W/out")"
grep -qx 'drift: keys: a login with the setup key is still running: session 8 (leader pid 6060)' "$W/out" && ok "a setup session whose leader ended is drift while a command runs" || bad "orphan session: $(cat "$W/out")"
grep -q 'session 9\|session 10\|connected: pid' "$W/out" && bad "another session or a bare pid is drift: $(grep '^drift: keys' "$W/out")" || ok "the build session and a reused leader pid are not drift"
: >"$R/.calls"
run "$R" apply
grep -qx 'terminate-session 7' "$R/.calls" && grep -qx 'terminate-session 8' "$R/.calls" && ok "apply ends both setup sessions" || bad "terminate calls: $(cat "$R/.calls")"
[ ! -e "$R/.state/session-procs/7" ] && [ ! -e "$R/.state/session-procs/8" ] && ok "no process of the setup sessions is left (5050, 5051, 5052, 6070)" || bad "left: $(cat "$R/.state/session-procs/7" "$R/.state/session-procs/8" 2>/dev/null)"
grep -q 'terminate-session 9\|terminate-session 10\|kill ' "$R/.calls" && bad "apply ended something else: $(cat "$R/.calls")" || ok "apply leaves the build session and the reused pid's session"
run "$R" check
[ $? -eq 0 ] && ok "after the setup sessions ended: 0 drift" || bad "after terminate: $(cat "$W/out")"
rm -rf "$R/.state/sshd-journal" "$R/.state/alive-pids" "$R/.state/sessions" "$R/.state/session-procs"

# 6. The bundle is one self-contained script that gives the same answer.
sh "$P" bundle "$W/node.conf" check >"$W/bundle.sh" 2>"$W/err"
st=$?
if [ $st -eq 0 ] && sh -n "$W/bundle.sh"; then ok "bundle is valid sh"; else bad "bundle exit $st: $(cat "$W/err")"; fi
NODE_TEST=1 NODE_ROOT=$R sh "$W/bundle.sh" >"$W/out" 2>"$W/err"
st=$?
if [ $st -eq 0 ] && grep -q '^0 drift$' "$W/out"; then ok "bundle check: 0 drift"; else bad "bundle check exit $st: $(cat "$W/out" "$W/err" | tail -5)"; fi
# The bundle carries the pins: the provision.sh inside it names the musl source pin, and the
# bundle names a musl-gcc link that is gone by the pin's directory.
sed -n "s/^printf '%s' '\(.*\)' | base64 -d .*/\1/p" "$W/bundle.sh" | base64 -d | tar -xzOf - provision.sh 2>/dev/null | grep -qx "MUSL_PIN='musl 1.2.6 d585fd3b613c66151fc3249e8ed44f77020cb5e6c1e635a616d3f9f82460512a https://musl.libc.org/releases/musl-1.2.6.tar.gz'" && ok "the bundle carries the musl pin" || bad "the bundle's provision.sh has no musl pin"
rm -f "$R/usr/local/bin/musl-gcc"
NODE_TEST=1 NODE_ROOT=$R sh "$W/bundle.sh" >"$W/out" 2>"$W/err"
st=$?
[ $st -eq 1 ] && grep -qx "drift: musl: /usr/local/bin/musl-gcc is 'missing', want a link to /usr/local/musl-1.2.6/bin/musl-gcc" "$W/out" && ok "bundle check names a missing musl-gcc link by the pin's directory" || bad "bundle and musl: exit $st $(cat "$W/out" "$W/err" | tail -5)"
ln -s /usr/local/musl-1.2.6/bin/musl-gcc "$R/usr/local/bin/musl-gcc"

# 6b. push.sh check compares only what a node cannot get from its packages alone: the rust, go and
# node lines of toolchains.sh and the release of each pinned tool. The C tools differ between
# machines and are not compared; nothing else is a file, so there is no file to compare.
U=$here/push.sh
# The node reports each pinned tool's release as `pin <name> <version>`; the workstation's lines
# for them (it may not have them) are not compared.
pins_ok=$(sh "$P" pins | sed 's/^/pin /')
printf 'host ws\nrust 1.99.1-x86_64-unknown-linux-gnu abc 111\ngo go1.24.0 linux/amd64\nnode v22.1.0\nc gcc gcc 13.2.0\nc clang clang 17\npin geckodriver missing\n' >"$W/here"
printf 'host node\nrust 1.99.1-x86_64-unknown-linux-gnu abc 111\ngo go1.24.0 linux/amd64\nnode v22.1.0\nc gcc gcc 11.4.0\nc clang missing\n%s\n' "$pins_ok" >"$W/node"
sh "$U" compare "$W/here" "$W/node" >"$W/out" 2>"$W/err"
st=$?
[ $st -eq 0 ] && grep -q '^0 drift$' "$W/out" && ok "equal rust, go and node lines: 0 drift, C tools not compared" || bad "equal lines: exit $st $(cat "$W/out" "$W/err")"
printf 'host node\nrust 1.99.1-x86_64-unknown-linux-gnu def 111\ngo go1.25.0 linux/amd64\n%s\n' "$pins_ok" >"$W/node"
sh "$U" compare "$W/here" "$W/node" >"$W/out" 2>"$W/err"
st=$?
[ $st -eq 1 ] && ok "compare with differences exits 1" || bad "compare with differences exit $st"
grep -qx 'drift: push: workstation has: rust 1.99.1-x86_64-unknown-linux-gnu abc 111' "$W/out" && grep -qx 'drift: push: only on the node: rust 1.99.1-x86_64-unknown-linux-gnu def 111' "$W/out" && ok "a rust toolchain of another commit is drift" || bad "rust: $(cat "$W/out")"
grep -qx 'drift: push: workstation has: go go1.24.0 linux/amd64' "$W/out" && grep -qx 'drift: push: only on the node: go go1.25.0 linux/amd64' "$W/out" && ok "another go version is drift" || bad "go: $(cat "$W/out")"
grep -qx 'drift: push: workstation has: node v22.1.0' "$W/out" && ok "a node missing on the node is drift" || bad "node: $(cat "$W/out")"
grep -q '^5 drift$' "$W/out" && ok "five differences: 5 drift" || bad "count: $(cat "$W/out")"
printf 'host ws\nrust 1.99.1-x aaa t1\nrust 1.98.1-x bbb t2\nnode v22.1.0\n' >"$W/here"
printf 'host node\nrust 1.99.1-x aaa t1\nrust 1.98.1-x bbb t2\nrust 1.85.0-x ccc t3\nnode v22.1.0\n%s\n' "$pins_ok" >"$W/node"
sh "$U" compare "$W/here" "$W/node" >"$W/out" 2>&1
st=$?
[ $st -eq 0 ] && grep -qx '0 drift' "$W/out" && ok "a rust toolchain only the node has (1.85.0) is not drift" || bad "node-only toolchain: exit $st $(cat "$W/out")"
printf 'host node\nrust 1.99.1-x aaa t1\nrust 1.85.0-x ccc t3\nnode v22.1.0\n%s\n' "$pins_ok" >"$W/node"
sh "$U" compare "$W/here" "$W/node" >"$W/out" 2>&1
st=$?
[ $st -eq 1 ] && grep -qx 'drift: push: workstation has: rust 1.98.1-x bbb t2' "$W/out" && grep -qx '1 drift' "$W/out" && ok "a workstation toolchain missing on the node is drift" || bad "missing toolchain: exit $st $(cat "$W/out")"
printf 'host node\nrust 1.99.1-x aaa t1\nrust 1.98.1-x bbb t9\nnode v22.1.0\n%s\n' "$pins_ok" >"$W/node"
sh "$U" compare "$W/here" "$W/node" >"$W/out" 2>&1
st=$?
[ $st -eq 1 ] && grep -qx 'drift: push: only on the node: rust 1.98.1-x bbb t9' "$W/out" && grep -qx '2 drift' "$W/out" && ok "the same toolchain with another line is drift on both sides" || bad "changed toolchain: exit $st $(cat "$W/out")"
printf 'host ws\ntool tsc 6.0.3\ntool esbuild 0.28.2\n' >"$W/here"
printf 'host node\ntool tsc 6.0.2\n%s\n' "$pins_ok" >"$W/node"
sh "$U" compare "$W/here" "$W/node" >"$W/out" 2>&1
st=$?
[ $st -eq 1 ] && grep -qx 'drift: push: workstation has: tool tsc 6.0.3' "$W/out" && grep -qx 'drift: push: only on the node: tool tsc 6.0.2' "$W/out" && grep -qx 'drift: push: workstation has: tool esbuild 0.28.2' "$W/out" && grep -qx '3 drift' "$W/out" && ok "a tool version that differs, and a tool missing on the node, are drift" || bad "tool lines: exit $st $(cat "$W/out")"
printf 'host node\ntool tsc 6.0.3\ntool esbuild 0.28.2\n%s\n' "$pins_ok" >"$W/node"
sh "$U" compare "$W/here" "$W/node" >"$W/out" 2>&1
st=$?
[ $st -eq 0 ] && grep -qx '0 drift' "$W/out" && ok "equal tool lines: 0 drift" || bad "equal tool lines: exit $st $(cat "$W/out")"
printf 'host ws\nrust a b c\n' >"$W/here"
printf 'host node\nrust a b c\n%s\n' "$(echo "$pins_ok" | sed 's/^pin geckodriver .*/pin geckodriver 0.36.0/')" >"$W/node"
sh "$U" compare "$W/here" "$W/node" >"$W/out" 2>&1
st=$?
[ $st -eq 1 ] && grep -qx "drift: push: pin geckodriver: node has 'geckodriver 0.36.0', want geckodriver 0.37.1" "$W/out" && ok "push.sh check names a pinned tool of another release on the node" || bad "pin drift: exit $st $(cat "$W/out")"
printf 'host node\nrust a b c\n%s\n' "$(echo "$pins_ok" | grep -v '^pin geckodriver ')" >"$W/node"
sh "$U" compare "$W/here" "$W/node" >"$W/out" 2>&1
st=$?
[ $st -eq 1 ] && grep -qx "drift: push: pin geckodriver: node has 'missing', want geckodriver 0.37.1" "$W/out" && ok "push.sh check names a pinned tool missing on the node" || bad "pin missing: exit $st $(cat "$W/out")"
printf 'host ws\n' >"$W/node"
sh "$U" compare "$W/here" "$W/node" >/dev/null 2>&1
[ $? -eq 2 ] && ok "a result from the workstation itself is refused" || bad "same host not refused"

# 6c. push.sh copies nothing: apply and bins are gone, check says so. The check job is `slot` run
# in the node's checkout; a stand-in slot runs the digest here and reports another host name.
mkdir -p "$W/home/.cargo/bin" "$W/bin"
# task prints the bare version (`3.54.0`), the other pinned tools `<name> <version>`.
sh "$P" pins | while read -r b v; do if [ "$b" = task ]; then printf '#!/bin/sh\necho "%s"\n' "$v"; else printf '#!/bin/sh\necho "%s %s"\n' "$b" "$v"; fi >"$W/home/.cargo/bin/$b"; chmod 755 "$W/home/.cargo/bin/$b"; done
printf '#!/bin/sh\n"$@" | sed "s/^host .*/host stand-in-node/"\n' >"$W/bin/slot"
chmod 755 "$W/bin/slot"
conf
sed -i "s|^BUILD_HOME=.*|BUILD_HOME=$W/home|" "$W/node.conf"
printf 'NODE_SSH=false\nNODE_HOST=node-1\nNODE_RUN_DIR=%s\n' "$W/home" >>"$W/node.conf"
for m in apply bins; do
  HOME=$W/home sh "$U" $m "$W/node.conf" >"$W/out" 2>"$W/err"
  st=$?
  [ $st -eq 2 ] && grep -qi "usage\|push.sh" "$W/err" && ok "push.sh $m is gone" || bad "push.sh $m still runs: exit $st $(cat "$W/out" "$W/err" | head -3)"
done
HOME=$W/home PATH=$W/bin:$PATH sh "$U" check "$W/node.conf" >"$W/out" 2>"$W/err"
st=$?
[ $st -eq 0 ] && grep -q '^0 drift$' "$W/out" && ok "push.sh check: equal machines print 0 drift" || bad "push check: exit $st $(cat "$W/out" "$W/err" | tail -4)"
grep -qx 'push: nothing is copied to the node; it installs everything itself (provision.sh apply)' "$W/out" && ok "push.sh check says that nothing is copied" || bad "no 'nothing is copied' line: $(cat "$W/out")"
grep -rq 'rsync' "$U" && bad "push.sh still mentions rsync" || ok "push.sh has no copy function"

# 6e. A git or ssh call on the node that asks for a credential prints what to do: fetch on the
# workstation; `slot --here` only for the one step that reads a credential while it runs, not a
# whole task; a step that needs a local service the node does not run goes in `.slot.toml`
# `local`. docker is not named there: the node runs it (section 6o).
for b in no-credentials ssh; do
  msg=$(sh "$R/srv/slot/nodebin/$b" host true 2>&1 >/dev/null)
  echo "$msg" | grep -q 'needs docker' && bad "$b: still sends a docker step to the workstation" || ok "$b: docker is not named as a reason to leave the node"
  echo "$msg" | grep -q 'only for the step that reads a credential' && ok "$b: --here only for a credential read" || bad "$b: $msg"
  echo "$msg" | grep -q '\.slot\.toml' && ok "$b: names .slot.toml local" || bad "$b: no .slot.toml in: $msg"
  echo "$msg" | grep -q 'or run this step with slot --here' && bad "$b: still offers --here for any step" || ok "$b: no --here for any step"
done

# 6g. One base ess makes one choice per repository, wherever it is installed: an exact pin runs
# that release from ess's own cache (here a stand-in for 0.99.1);
# a line pin the base cannot serve fails with ess's own line, and runs no other release. Needs
# an ess on PATH (the workstation has one; a CI runner without it skips).
if command -v ess >/dev/null 2>&1 && base=$(ess --version 2>/dev/null | sed -n 's/^ess //p') && [ -n "$base" ]; then
  E=$W/ess
  mkdir -p "$E/tc/0.99.1" "$E/exact" "$E/line" "$E/same"
  printf '#!/bin/sh\ncase "$1" in --version) echo "ess 0.99.1" ;; *) echo "ran 0.99.1" ;; esac\n' >"$E/tc/0.99.1/ess"
  chmod 755 "$E/tc/0.99.1/ess"
  for d in exact:0.99.1 line:9.9 same:${base%.*}; do
    printf 'format: ess-inputs/2\nrequires: ess %s\nspecification:\n  - system.yaml\nscenarios: []\n' "${d#*:}" >"$E/${d%%:*}/ess-inputs.yaml"
    : >"$E/${d%%:*}/system.yaml"
  done
  (cd "$E/exact" && ESS_TOOLCHAIN_DIR=$E/tc ESS_TOOLCHAIN_QUIET=1 ess specify validate) >"$W/out" 2>&1
  grep -qx 'ran 0.99.1' "$W/out" && ok "base ess $base with an exact pin 0.99.1 runs 0.99.1" || bad "exact pin: $(head -3 "$W/out")"
  (cd "$E/line" && ESS_TOOLCHAIN_DIR=$E/tc ess specify validate) >"$W/out" 2>&1
  st=$?
  [ $st -ne 0 ] && grep -q "requires ess 9.9 and this is ess $base" "$W/out" && ! grep -q 'ran 0.99.1' "$W/out" &&
    ok "a line pin base ess $base cannot serve fails with ess's own line" || bad "line pin: exit $st $(head -3 "$W/out")"
  (cd "$E/same" && ESS_TOOLCHAIN_DIR=$E/tc ess toolchain which) >"$W/out" 2>&1
  head -1 "$W/out" | grep -qx "ess $base" && ok "the line pin ${base%.*} runs the base ess $base" || bad "same line: $(head -3 "$W/out")"
else
  echo "ok - skipped: no ess on PATH"
fi

# 6i. The real download paths (slot, pinned tools, rustup-init, archives) refuse a file whose
# SHA-256 is not the expected one: nothing is unpacked and nothing downloaded runs.
# NODE_TEST_REAL_FETCH=1 makes the four fetch functions take their real branch while every other
# system query and action stays a stub; `curl` and `tar` on PATH are stand-ins that serve a file
# that is not what the pin says and record their calls.
conf
mkdir -p "$W/stubbin"
cat >"$W/stubbin/curl" <<EOF
#!/bin/sh
echo "\$*" >>"$W/curl.log"
out=
url=
while [ \$# -gt 0 ]; do
  case \$1 in
    -fsSLo) out=\$2; shift ;;
    -*) ;;
    *) url=\$1 ;;
  esac
  shift
done
[ -n "\$out" ] || out=\${url##*/}
case \$out in
  SHA256SUMS) printf '%s  slot-0.36.0-x86_64-unknown-linux-gnu.tar.gz\n' "\$(cat "$W/slot-sum")" >"\$out" ;;
  *) printf '#!/bin/sh\ntouch "$W/ran"\n' >"\$out" ;;
esac
EOF
printf '#!/bin/sh\necho "$*" >>"%s/tar.log"\nexit 1\n' "$W" >"$W/stubbin/tar"
chmod 755 "$W/stubbin/curl" "$W/stubbin/tar"
realrun() { NODE_TEST=1 NODE_TEST_REAL_FETCH=1 NODE_ROOT=$1 PATH=$W/stubbin:$PATH sh "$P" apply "$W/node.conf" >"$W/out" 2>"$W/err"; }
RF=$W/r6
mkdir -p "$RF"
printf '%064d\n' 0 >"$W/slot-sum"
: >"$W/curl.log"
: >"$W/tar.log"
rm -f "$W/ran"
realrun "$RF"
st=$?
[ $st -eq 1 ] && ok "downloads with a wrong SHA-256: apply exits 1" || bad "wrong SHA-256: apply exit $st"
for part in slot tool:wasm-bindgen tool:geckodriver tool:sccache tool:cargo-nextest tool:task tool:ess tool:cargo-about rustup archive:go archive:node archive:typescript archive:prettier archive:esbuild archive:pnpm archive:yarn archive:yq archive:helm make musl home:ess@0.97.4 home:@types/node@26.3.0 home:undici-types@8.3.0 home:aep@0.80.0 home:groovy@2.4.10; do
  grep -q "^failed: $part: " "$W/err" && ok "wrong SHA-256: $part fails" || bad "wrong SHA-256: no failed line for $part: $(grep '^failed' "$W/err" | head -3)"
done
[ ! -s "$W/tar.log" ] && ok "wrong SHA-256: tar never runs" || bad "tar ran on an unchecked file: $(cat "$W/tar.log")"
[ ! -e "$W/ran" ] && ok "wrong SHA-256: no downloaded file runs" || bad "a downloaded file ran"
[ "$(grep -c 'rustup-init\|go1.27.1\|node-v22\|task_linux\|nextest\|geckodriver\|wasm-bindgen\|sccache\|slot-0.36\|ess-0.99.2\|cargo-about-0.9.1\|yq_linux_amd64' "$W/curl.log")" -ge 12 ] && ok "wrong SHA-256: every download was attempted" || bad "curl calls: $(cat "$W/curl.log")"
grep -q 'musl-1.2.6.tar.gz' "$W/curl.log" && ok "wrong SHA-256: the musl source was asked for" || bad "no musl download in: $(cat "$W/curl.log")"
grep -q '^failed: dash: download, checksum or build of dash 0.5.12' "$W/err" && grep -q 'dash-0.5.12.tar.gz' "$W/curl.log" && ok "wrong SHA-256: the dash source was asked for, and dash fails" || bad "wrong SHA-256: dash: $(grep '^failed: dash' "$W/err") $(grep dash "$W/curl.log")"
# Control: with the SHA256SUMS line that matches the served file, tar is reached (so the stand-in
# tar would record a call if the check had failed open above), with no owner taken from the archive.
RG=$W/r6b
mkdir -p "$RG"
printf '#!/bin/sh\ntouch "%s/ran"\n' "$W" | sha256sum | cut -d' ' -f1 >"$W/slot-sum"
: >"$W/tar.log"
# The same control for the musl source: with a pin that carries the SHA-256 of the served file
# its tar is reached; make's source, served as the same file under make's own pin, is not.
export NODE_TEST_MUSL_PIN="musl 1.2.6 $(cat "$W/slot-sum") https://musl.libc.org/releases/musl-1.2.6.tar.gz"
realrun "$RG"
unset NODE_TEST_MUSL_PIN
[ "$(grep -c -- '-xf src.tar.gz -C src --strip-components=1 --no-same-owner' "$W/tar.log")" -eq 1 ] && ok "control: a musl source with the SHA-256 of its pin reaches tar, once, with no owner taken from the archive" || bad "control: musl tar: $(cat "$W/tar.log")"
grep -q 'xzf slot-0.36.0-x86_64-unknown-linux-gnu.tar.gz' "$W/tar.log" && ok "control: a matching SHA-256 reaches tar" || bad "control: tar not reached: $(cat "$W/tar.log")"
grep -q -- '--no-same-owner' "$W/tar.log" && ok "slot's tar takes no owner from the archive" || bad "slot's tar has no --no-same-owner: $(cat "$W/tar.log")"
# SLOT_TARGET=x86_64-unknown-linux-musl through the real download branch: the archive of that
# name is asked for, and its checksum is the line of that name in the release's SHA256SUMS. A
# release whose SHA256SUMS has no such line (one before the musl archives) installs nothing; with
# the line that matches the served file, tar is reached with the musl archive.
RM=$W/r6m
mkdir -p "$RM" "$W/stubbin-musl"
echo 'SLOT_TARGET=x86_64-unknown-linux-musl' >>"$W/node.conf"
: >"$W/curl.log"
: >"$W/tar.log"
realrun "$RM"
grep -q 'releases/download/0.36.0/slot-0.36.0-x86_64-unknown-linux-musl.tar.gz' "$W/curl.log" && ! grep -q 'slot-0.36.0-x86_64-unknown-linux-gnu' "$W/curl.log" && ok "SLOT_TARGET musl: the musl archive is asked for, not the gnu one" || bad "musl download: $(grep 'slot-0.36.0' "$W/curl.log" | tr '\n' ' ')"
grep -q '^failed: slot: download or checksum of slot 0.36.0' "$W/err" && ! grep -q 'slot-0.36.0' "$W/tar.log" && ok "SLOT_TARGET musl: a SHA256SUMS without the line of the musl archive installs nothing, and tar never runs on it" || bad "musl without a checksum line: $(grep '^failed: slot' "$W/err") $(grep slot "$W/tar.log")"
sed 's/slot-0\.36\.0-x86_64-unknown-linux-gnu\.tar\.gz/slot-0.36.0-x86_64-unknown-linux-musl.tar.gz/' "$W/stubbin/curl" >"$W/stubbin-musl/curl"
chmod 755 "$W/stubbin-musl/curl"
: >"$W/tar.log"
NODE_TEST=1 NODE_TEST_REAL_FETCH=1 NODE_ROOT=$RM PATH=$W/stubbin-musl:$W/stubbin:$PATH sh "$P" apply "$W/node.conf" >"$W/out" 2>"$W/err"
grep -q 'xzf slot-0.36.0-x86_64-unknown-linux-musl.tar.gz --no-same-owner' "$W/tar.log" && ok "SLOT_TARGET musl: with the SHA256SUMS line of the musl archive, tar is reached with it and takes no owner from it" || bad "musl control: $(grep slot "$W/tar.log") $(grep '^failed: slot' "$W/err")"
conf
[ "$(grep -c 'no-same-owner' "$here/provision.sh")" -ge 3 ] && ok "every tar of provision.sh that runs as root has --no-same-owner" || bad "--no-same-owner count: $(grep -c 'no-same-owner' "$here/provision.sh")"
[ "$(grep -c 'u-s,g-s' "$here/provision.sh")" -ge 3 ] && ok "unpacked trees lose setuid and setgid (static check)" || bad "u-s,g-s count: $(grep -c 'u-s,g-s' "$here/provision.sh")"

# 6i-2. A home pin through the real download branch: a served tarball whose SHA-256 matches is
# unpacked, its files lose group/other write and setuid, the tree carries .slot-sha256, and check
# then reports no drift. A marker that differs from the pin is drift.
mkdir -p "$W/hsrc/ess-9.9.9-x" "$W/stubbin2"
printf '#!/bin/sh\necho "ess 9.9.9"\n' >"$W/hsrc/ess-9.9.9-x/ess"
printf 'readme\n' >"$W/hsrc/ess-9.9.9-x/README.md"
chmod 4777 "$W/hsrc/ess-9.9.9-x/ess"
chmod 666 "$W/hsrc/ess-9.9.9-x/README.md"
tar -czf "$W/home.tar.gz" -C "$W/hsrc" ess-9.9.9-x
HSHA=$(sha256sum "$W/home.tar.gz" | cut -d' ' -f1)
cat >"$W/stubbin2/curl" <<EOF
#!/bin/sh
out=
url=
while [ \$# -gt 0 ]; do
  case \$1 in
    -fsSLo) out=\$2; shift ;;
    -*) ;;
    *) url=\$1 ;;
  esac
  shift
done
case \$url in
  https://home.test/ess.tar.gz) cp "$W/home.tar.gz" "\$out" ;;
  *) printf 'junk\n' >"\$out" ;;
esac
EOF
chmod 755 "$W/stubbin2/curl"
HLINE="ess 9.9.9 $HSHA https://home.test/ess.tar.gz 1 .local/share/ess-versions/9.9.9 ess"
homerun() { NODE_TEST=1 NODE_TEST_REAL_FETCH=1 NODE_TEST_HOME_PINS="$HLINE" NODE_ROOT=$RH PATH=$W/stubbin2:$PATH sh "$P" "$1" "$W/node.conf" >"$W/out" 2>"$W/err"; }
RH=$W/rh
mkdir -p "$RH"
homerun apply
HT=$RH/var/lib/builder/.local/share/ess-versions/9.9.9
grep -q 'changed: home:ess@9.9.9' "$W/out" && ok "home pin, real branch: apply installs the tree" || bad "home real branch: $(cat "$W/out" "$W/err")"
[ "$(stat -c %a "$HT/ess" 2>/dev/null)" = 755 ] && ok "home pin: ess is mode 755 (setuid and group/other write dropped)" || bad "home ess mode: $(stat -c %a "$HT/ess" 2>&1)"
[ -z "$(find "$HT" -perm /022 -o -perm /6000 2>/dev/null)" ] && ok "home pin: no group- or other-writable and no setuid file" || bad "home pin writable files: $(find "$HT" -perm /022 -o -perm /6000)"
[ "$(cat "$HT/.slot-sha256" 2>/dev/null)" = "$HSHA" ] && ok "home pin: .slot-sha256 holds the archive's SHA-256" || bad "home marker: $(cat "$HT/.slot-sha256" 2>&1)"
homerun check
grep -q 'home:ess@9.9.9' "$W/out" && bad "home pin: drift after a matching install: $(cat "$W/out")" || ok "home pin: check reports no drift after the install"
printf 'other\n' >"$HT/.slot-sha256"
homerun check
grep -q 'archive' "$W/out" && ok "home pin: a marker of another archive is drift" || bad "home marker drift: $(cat "$W/out")"
homerun apply
[ "$(cat "$HT/.slot-sha256" 2>/dev/null)" = "$HSHA" ] && [ ! -e "$HT.slot-new" ] && [ ! -e "$HT.slot-old" ] && ok "home pin: apply replaces a tree of another archive, no .slot-new or .slot-old left" || bad "home repair: $(ls -a "$RH/var/lib/builder/.local/share/ess-versions")"

# A zip pin (Playwright browser) through the real branch: unzipped without stripping, both markers,
# the version read from `--version`. Needs zip and unzip.
if command -v zip >/dev/null 2>&1 && command -v unzip >/dev/null 2>&1; then
  mkdir -p "$W/zsrc/chrome-headless-shell-linux64"
  printf '#!/bin/sh\necho "Google Chrome for Testing 7.7.7.7"\n' >"$W/zsrc/chrome-headless-shell-linux64/chrome-headless-shell"
  chmod 4777 "$W/zsrc/chrome-headless-shell-linux64/chrome-headless-shell"
  (cd "$W/zsrc" && zip -qr "$W/home.zip" chrome-headless-shell-linux64)
  ZSHA=$(sha256sum "$W/home.zip" | cut -d' ' -f1)
  cat >"$W/stubbin2/curl" <<EOF
#!/bin/sh
out=
url=
while [ \$# -gt 0 ]; do
  case \$1 in
    -fsSLo) out=\$2; shift ;;
    -*) ;;
    *) url=\$1 ;;
  esac
  shift
done
case \$url in
  https://home.test/shell.zip) cp "$W/home.zip" "\$out" ;;
  *) printf 'junk\n' >"\$out" ;;
esac
EOF
  HLINE="chrome-headless-shell 7.7.7.7 $ZSHA https://home.test/shell.zip zip .cache/ms-playwright/chromium_headless_shell-9999 chrome-headless-shell-linux64/chrome-headless-shell"
  RZ=$W/rz
  mkdir -p "$RZ"
  homerun() { NODE_TEST=1 NODE_TEST_REAL_FETCH=1 NODE_TEST_HOME_PINS="$HLINE" NODE_ROOT=$RZ PATH=$W/stubbin2:$PATH sh "$P" "$1" "$W/node.conf" >"$W/out" 2>"$W/err"; }
  homerun apply
  ZT=$RZ/var/lib/builder/.cache/ms-playwright/chromium_headless_shell-9999
  grep -q 'changed: home:chrome-headless-shell@7.7.7.7' "$W/out" && ok "zip pin, real branch: apply installs the browser" || bad "zip real branch: $(cat "$W/out" "$W/err")"
  [ -e "$ZT/INSTALLATION_COMPLETE" ] && [ -e "$ZT/DEPENDENCIES_VALIDATED" ] && ok "zip pin: both Playwright markers exist" || bad "zip markers: $(ls -a "$ZT")"
  [ "$(stat -c %a "$ZT/chrome-headless-shell-linux64/chrome-headless-shell" 2>/dev/null)" = 755 ] && ok "zip pin: the binary is mode 755" || bad "zip binary mode: $(stat -c %a "$ZT/chrome-headless-shell-linux64/chrome-headless-shell" 2>&1)"
  homerun check
  grep -q 'home:chrome' "$W/out" && bad "zip pin: drift after a matching install: $(cat "$W/out")" || ok "zip pin: check reports no drift after the install"
  # A zip1 pin (one top directory, stripped) through the real branch: the tree is the content of
  # the top directory, no Playwright marker, a script keeps its exec bit, the check is the jar.
  mkdir -p "$W/gsrc/groovy-9.9/embeddable" "$W/gsrc/groovy-9.9/bin"
  echo jar >"$W/gsrc/groovy-9.9/embeddable/groovy-all-9.9.jar"
  printf '#!/bin/sh\necho groovyc\n' >"$W/gsrc/groovy-9.9/bin/groovyc"
  chmod 4777 "$W/gsrc/groovy-9.9/bin/groovyc"
  (cd "$W/gsrc" && zip -qr "$W/groovy.zip" groovy-9.9)
  GSHA=$(sha256sum "$W/groovy.zip" | cut -d' ' -f1)
  mkdir -p "$W/g2src/a" "$W/g2src/b/embeddable"
  echo jar >"$W/g2src/b/embeddable/groovy-all-9.9.jar"
  (cd "$W/g2src" && zip -qr "$W/groovy2.zip" a b)
  G2SHA=$(sha256sum "$W/groovy2.zip" | cut -d' ' -f1)
  mkdir -p "$W/g3src"
  echo jar >"$W/g3src/groovy-all-9.9.jar"
  (cd "$W/g3src" && zip -qr "$W/groovy3.zip" groovy-all-9.9.jar)
  G3SHA=$(sha256sum "$W/groovy3.zip" | cut -d' ' -f1)
  cat >"$W/stubbin2/curl" <<EOF
#!/bin/sh
out=
url=
while [ \$# -gt 0 ]; do
  case \$1 in
    -fsSLo) out=\$2; shift ;;
    -*) ;;
    *) url=\$1 ;;
  esac
  shift
done
case \$url in
  https://home.test/groovy.zip) cp "$W/groovy.zip" "\$out" ;;
  https://home.test/groovy2.zip) cp "$W/groovy2.zip" "\$out" ;;
  https://home.test/groovy3.zip) cp "$W/groovy3.zip" "\$out" ;;
  *) printf 'junk\n' >"\$out" ;;
esac
EOF
  HLINE="groovy 9.9 $GSHA https://home.test/groovy.zip zip1 .sdkman/candidates/groovy/9.9 embeddable/groovy-all-9.9.jar"
  RZ=$W/rg
  mkdir -p "$RZ"
  homerun apply
  GT=$RZ/var/lib/builder/.sdkman/candidates/groovy/9.9
  grep -q 'changed: home:groovy@9.9' "$W/out" && [ -f "$GT/embeddable/groovy-all-9.9.jar" ] && [ ! -e "$GT/groovy-9.9" ] && ok "zip1 pin, real branch: apply installs the tree without the top directory" || bad "zip1 real branch: $(cat "$W/out" "$W/err"; ls -a "$GT" 2>&1)"
  [ ! -e "$GT/INSTALLATION_COMPLETE" ] && [ ! -e "$GT/DEPENDENCIES_VALIDATED" ] && ok "zip1 pin: no Playwright marker" || bad "zip1 markers: $(ls -a "$GT")"
  [ "$(stat -c %a "$GT/bin/groovyc" 2>/dev/null)" = 755 ] && ok "zip1 pin: bin/groovyc is mode 755" || bad "zip1 groovyc mode: $(stat -c %a "$GT/bin/groovyc" 2>&1)"
  homerun check
  grep -q 'home:groovy' "$W/out" && bad "zip1 pin: drift after a matching install: $(cat "$W/out")" || ok "zip1 pin: check reports no drift after the install"
  HLINE="groovy 9.8 $G2SHA https://home.test/groovy2.zip zip1 .sdkman/candidates/groovy/9.8 embeddable/groovy-all-9.9.jar"
  homerun apply
  st=$?
  [ $st -eq 1 ] && grep -qx 'failed: home:groovy@9.8: download or checksum of groovy 9.8' "$W/err" && [ ! -e "$RZ/var/lib/builder/.sdkman/candidates/groovy/9.8" ] && ok "zip1 pin: a zip with two top entries is refused, no tree" || bad "zip1 two tops: exit $st $(cat "$W/out" "$W/err")"
  # The same well-formed zip that installed above, under another SHA-256: only the checksum refuses it.
  HLINE="groovy 9.7 $G2SHA https://home.test/groovy.zip zip1 .sdkman/candidates/groovy/9.7 embeddable/groovy-all-9.9.jar"
  homerun apply
  st=$?
  [ $st -eq 1 ] && grep -qx 'failed: home:groovy@9.7: download or checksum of groovy 9.7' "$W/err" && [ ! -e "$RZ/var/lib/builder/.sdkman/candidates/groovy/9.7" ] && ok "zip1 pin: a well-formed zip with another SHA-256 is refused, no tree" || bad "zip1 wrong sha: exit $st $(cat "$W/out" "$W/err")"
  # One top entry that is a file, not a directory.
  HLINE="groovy 9.6 $G3SHA https://home.test/groovy3.zip zip1 .sdkman/candidates/groovy/9.6 embeddable/groovy-all-9.9.jar"
  homerun apply
  st=$?
  [ $st -eq 1 ] && grep -qx 'failed: home:groovy@9.6: download or checksum of groovy 9.6' "$W/err" && [ ! -e "$RZ/var/lib/builder/.sdkman/candidates/groovy/9.6" ] && ok "zip1 pin: a zip whose one top entry is a file is refused, no tree" || bad "zip1 top file: exit $st $(cat "$W/out" "$W/err")"
else
  ok "zip pin: skipped, zip or unzip missing on this host"
fi

# 6j. After apply changed a unit file or replaced the slot binary, each running unit it affects
# restarts when no job runs (else a pending line): a restart of slot-node ends the jobs, and one
# of sccache-node ends the compilers the cache server runs for them. check reports a running unit
# whose loaded definition, process environment or slot binary is not what its file and the config
# say.
conf
R7=$W/r7
mkdir -p "$R7"
run "$R7" apply
run "$R7" check
[ $? -eq 0 ] && ok "units started by apply match their files: 0 drift" || bad "after the first apply: $(cat "$W/out")"
echo 'Environment=EXTRA=1' >>"$R7/etc/systemd/system/sccache-node.service"
sed -i 's/^GO_PRIVATE=.*/GO_PRIVATE=git.other.org/' "$W/node.conf"
echo 1 >"$R7/.state/jobs"
: >"$R7/.calls"
run "$R7" apply
grep -qx 'systemctl restart sccache-node.service' "$R7/.calls" && bad "sccache-node restarted with a job running" || ok "a changed unit file: sccache-node waits while a job runs"
grep -qx 'systemctl restart slot-node.service' "$R7/.calls" && bad "slot-node restarted with a job running" || ok "a changed unit file: slot-node waits while a job runs"
grep -q '^pending: running: slot-node restarts when no job runs (1 now): ' "$W/err" && ok "slot-node's restart is a pending line" || bad "no pending line: $(cat "$W/err")"
grep -qx 'pending: running: sccache-node restarts when no job runs (1 now): its unit file changed' "$W/err" && ok "sccache-node's restart is a pending line" || bad "no pending line for sccache-node: $(cat "$W/err")"
run "$R7" check
st=$?
[ $st -eq 1 ] && grep -q '^drift: running: slot-node.service: ' "$W/out" && ok "check names slot-node running an older definition" || bad "stale slot-node not named: exit $st $(cat "$W/out")"
echo 0 >"$R7/.state/jobs"
: >"$R7/.calls"
run "$R7" apply
grep -qx 'systemctl restart slot-node.service' "$R7/.calls" && ok "with no job running slot-node restarts" || bad "no slot-node restart: $(cat "$R7/.calls")"
run "$R7" check
[ $? -eq 0 ] && ok "after the restarts: 0 drift" || bad "after the restarts: $(cat "$W/out")"
echo 'Environment=OLD=1' >"$R7/.state/loaded/sccache-node.service"
run "$R7" check
grep -q '^drift: running: sccache-node.service: .*loaded' "$W/out" && ok "check names a loaded definition that is not the unit file" || bad "loaded definition: $(cat "$W/out")"
: >"$R7/.calls"
run "$R7" apply
grep -qx 'systemctl restart sccache-node.service' "$R7/.calls" && ok "apply restarts the unit whose loaded definition differs" || bad "no restart: $(cat "$R7/.calls")"
echo 'slot 0.35.0' >"$R7/.state/running-version/slot-node.service"
run "$R7" check
grep -q "^drift: running: slot-node.service: .*slot 0.35.0.*slot 0.36.0" "$W/out" && ok "check names a running slot of another version" || bad "running slot version: $(cat "$W/out")"
run "$R7" apply
run "$R7" check
[ $? -eq 0 ] && ok "after the slot restart: 0 drift" || bad "after the slot restart: $(cat "$W/out")"
printf '#!/bin/sh\necho slot 0.35.0\n' >"$R7/srv/slot/bin/slot"
: >"$R7/.calls"
run "$R7" apply
grep -qx 'changed: slot: slot 0.36.0 installed at /srv/slot/bin/slot' "$W/out" && grep -qx 'systemctl restart slot-node.service' "$R7/.calls" && ok "a replaced slot binary restarts slot-node" || bad "replaced binary: $(cat "$W/out" "$R7/.calls")"

# 6k. The user's commands run with a clean environment: nothing of the provisioning run's own
# variables reaches them, and PATH is the units' PATH.
conf
R8=$W/r8
mkdir -p "$R8"
run "$R8" apply
printf '#!/bin/sh\nenv >"%s/env.log"\necho "geckodriver 0.37.1"\n' "$W" >"$R8/srv/slot/cargo/bin/geckodriver"
chmod 755 "$R8/srv/slot/cargo/bin/geckodriver"
rm -f "$W/env.log"
NODE_TEST=1 NODE_ROOT=$R8 SLOT_LEAK=leaked sh "$P" check "$W/node.conf" >"$W/out" 2>"$W/err"
[ -f "$W/env.log" ] && ok "a user command ran under check" || bad "no user command ran: $(cat "$W/err")"
grep -q '^SLOT_LEAK=\|^NODE_ROOT=\|^NODE_TEST=' "$W/env.log" && bad "provisioning variables reach a user command: $(grep '^SLOT_LEAK=\|^NODE_ROOT=\|^NODE_TEST=' "$W/env.log")" || ok "variables of the provisioning run are not visible to a user command"
grep -qx 'HOME=/var/lib/builder' "$W/env.log" && grep -q '^PATH=/srv/slot/nodebin:/var/lib/builder/.cargo/bin:/usr/local/bin:/usr/bin:' "$W/env.log" && ok "a user command gets HOME and the units' PATH" || bad "user environment: $(cat "$W/env.log")"

# 6l. The metadata service is closed before the build key goes in and before anything runs as the
# build user; a start that does not close it is a `failed:` line, exit 1, and no key.
conf
R9=$W/r9
mkdir -p "$R9"
: >"$R9/.state-imds-stays-open"
run "$R9" apply
st=$?
[ $st -eq 1 ] && grep -q '^failed: imds: ' "$W/err" && ok "an imds start that does not close it: failed line, exit 1" || bad "imds still open: exit $st $(cat "$W/err" | tail -3)"
[ -e "$R9/var/lib/builder/.ssh/authorized_keys" ] && bad "the build key was installed with the metadata service open" || ok "no key while the metadata service is open"
grep -q 'rustup-init\|fetch https://github.com/wasm-bindgen' "$R9/.calls" && bad "a download ran as the build user's installer with the metadata service open" || ok "no rustup or pinned tool install while the metadata service is open"
rm -f "$R9/.state-imds-stays-open"
run "$R9" apply
run "$R9" check
[ $? -eq 0 ] && [ -e "$R9/var/lib/builder/.ssh/authorized_keys" ] && ok "once it closes, apply finishes: key in, 0 drift" || bad "after imds closes: $(cat "$W/out" "$W/err" | tail -4)"

# 6m. Components and targets are made good on a toolchain that is already there: check names a
# missing one, apply adds only that one.
conf
printf 'RUST_COMPONENTS=clippy rustfmt\nRUST_TARGETS=wasm32-unknown-unknown\n' >>"$W/node.conf"
RA=$W/r10
mkdir -p "$RA"
run "$RA" apply
run "$RA" check
[ $? -eq 0 ] && ok "components and targets installed with the toolchain: 0 drift" || bad "components after install: $(cat "$W/out")"
td=$RA/srv/slot/rustup/toolchains/1.99.1-x86_64-unknown-linux-gnu
printf 'clippy-x86_64-unknown-linux-gnu\nrust-std-x86_64-unknown-linux-gnu\n' >"$td/.components"
: >"$td/.targets"
run "$RA" check
grep -qx 'drift: rust:1.99.1: component rustfmt missing' "$W/out" && grep -qx 'drift: rust:1.99.1: target wasm32-unknown-unknown missing' "$W/out" && ok "check names a missing component and a missing target" || bad "component drift: $(grep rust "$W/out")"
grep -q 'rust:stable\|component clippy' "$W/out" && bad "check names what is there: $(grep 'rust:stable\|clippy' "$W/out")" || ok "check names only what is missing"
: >"$RA/.calls"
run "$RA" apply
grep -qx 'rustup component add --toolchain 1.99.1 rustfmt' "$RA/.calls" && grep -qx 'rustup target add --toolchain 1.99.1 wasm32-unknown-unknown' "$RA/.calls" && ok "apply adds the missing component and target" || bad "add calls: $(cat "$RA/.calls")"
grep -q 'toolchain install\|add --toolchain stable\|add --toolchain 1.99.1 clippy' "$RA/.calls" && bad "apply did more than add the missing ones: $(cat "$RA/.calls")" || ok "apply adds only the missing ones"
run "$RA" check
[ $? -eq 0 ] && ok "after the adds: 0 drift" || bad "after the adds: $(cat "$W/out")"

# 6n. EXTRA_PACKAGES and the RUST_* names are refused when they could carry an option or a shell
# word (cases in section 1); push.sh prints nothing a node could use to move a terminal.
printf 'host ws\nrust a b c\n' >"$W/here"
printf 'host node\nrust a b c\ngo go1.0\033[31mred\033]0;title\007\nnode v1\n%s\n' "$(sh "$P" pins | sed 's/^/pin /')" >"$W/node"
sh "$here/push.sh" compare "$W/here" "$W/node" >"$W/out" 2>&1
tr -d '\000-\010\013-\037\177' <"$W/out" | cmp -s - "$W/out" && ok "push.sh prints no control character from the node's output" || bad "control characters in the push.sh output"
grep -q 'only on the node: go go1.0.*red' "$W/out" && ok "the printable part of the node's line is still shown" || bad "drift line lost: $(cat "$W/out" | head -3)"
grep -q 'version parity report, not an integrity check' "$here/README.md" && ok "README says push.sh check is not an integrity check" || bad "README lacks the parity-report line"

# 6o. docker: the package, docker.service enabled and started behind the rule that keeps a
# container off the metadata service, the build user in group docker, slot-node restarted when
# its process started before the group, and a timer that keeps what docker stores within
# DOCKER_GB.
conf
RD=$W/r11
mkdir -p "$RD"
run "$RD" check
grep -q '^drift: packages: .* docker' "$W/out" && grep -qx "drift: docker: $me is not in group docker" "$W/out" && ok "check names the docker package and the missing group on a bare node" || bad "docker drift: $(grep -i docker "$W/out" | cut -c1-300)"
run "$RD" apply
st=$?
[ $st -eq 0 ] && grep -qx "changed: docker: $me added to group docker" "$W/out" && ok "apply adds the build user to group docker" || bad "docker apply: exit $st $(grep -i docker "$W/out" "$W/err" | cut -c1-300)"
grep -qx docker "$RD/.state/packages" && ok "apply installs the docker package" || bad "docker not in the packages: $(tr '\n' ' ' <"$RD/.state/packages")"
grep -qx "$me:docker" "$RD/.state/groups" && ok "the group change is recorded" || bad "groups: $(cat "$RD/.state/groups" 2>&1)"
for u in docker.service slot-imds-containers.service slot-docker-prune.timer; do
  [ -f "$RD/.state/enabled/$u" ] && [ -f "$RD/.state/active/$u" ] && ok "$u is enabled and started" || bad "$u: enabled $(ls "$RD/.state/enabled" | tr '\n' ' ') active $(ls "$RD/.state/active" | tr '\n' ' ')"
done
[ -f "$RD/.state/enabled/slot-docker-prune.service" ] && bad "the prune service is enabled beside its timer" || ok "the prune service runs only from its timer"
dd=$RD/etc/systemd/system/docker.service.d/slot-imds.conf
grep -qx 'Requires=slot-imds-containers.service' "$dd" && grep -qx 'After=slot-imds-containers.service' "$dd" && ok "docker.service requires the container rule and starts after it" || bad "docker drop-in: $(cat "$dd" 2>&1)"
cu=$RD/etc/systemd/system/slot-imds-containers.service
grep -q '^ExecStart=.*iptables -w -I DOCKER-USER 1 -d 169.254.169.254/32 -j REJECT' "$cu" && grep -q '^ExecStart=.*ip6tables -w -I DOCKER-USER 1 -d fd00:ec2::254/128 -j REJECT' "$cu" && ok "the container rule rejects both metadata addresses in DOCKER-USER" || bad "container rule: $(grep ExecStart "$cu")"
grep -q '^Before=.*docker.service' "$cu" && ok "the container rule is ordered before docker.service" || bad "container rule order: $(grep Before "$cu")"
pu_pre=$RD/etc/systemd/system
grep -q "^ExecStop=" "$cu" && bad "stopping the container rule unit takes the rule away: $(grep ExecStop "$cu")" || ok "the container rule has no ExecStop: a stop leaves the rule in place"
grep -qx "Nice=19" "$pu_pre/slot-docker-prune.service" && grep -qx "IOSchedulingClass=idle" "$pu_pre/slot-docker-prune.service" && grep -qx "TimeoutStartSec=300" "$pu_pre/slot-docker-prune.service" && ok "the prune unit runs at idle priority with a start timeout" || bad "prune unit priority: $(cat "$pu_pre/slot-docker-prune.service")"
pu=$RD/etc/systemd/system/slot-docker-prune.service
grep -qx 'ExecStart=/bin/sh /usr/local/libexec/slot-node/docker-prune.sh 10' "$pu" && ok "the prune unit runs the script with the default cap of 10 GB" || bad "prune unit: $(grep ExecStart "$pu")"
grep -qx 'OnUnitActiveSec=600' "$RD/etc/systemd/system/slot-docker-prune.timer" && ok "the prune timer runs every 10 min" || bad "prune timer: $(cat "$RD/etc/systemd/system/slot-docker-prune.timer")"
grep -qw docker "$RD/.state/procgroups/slot-node.service" && ok "slot-node, started after the group change, has group docker" || bad "slot-node groups: $(cat "$RD/.state/procgroups/slot-node.service" 2>&1)"
run "$RD" check
st=$?
[ $st -eq 0 ] && grep -q '^0 drift$' "$W/out" && ok "docker: 0 drift after one apply" || bad "docker: exit $st $(cat "$W/out")"
# The cap is a config key.
echo 'DOCKER_GB=7' >>"$W/node.conf"
run "$RD" check
grep -q '^drift: unit:slot-docker-prune.service' "$W/out" && ok "a changed DOCKER_GB is drift in the prune unit" || bad "DOCKER_GB=7: $(cat "$W/out")"
run "$RD" apply
grep -qx 'ExecStart=/bin/sh /usr/local/libexec/slot-node/docker-prune.sh 7' "$pu" && ok "apply writes the new cap into the prune unit" || bad "prune unit after DOCKER_GB=7: $(grep ExecStart "$pu")"
conf
echo 'DOCKER_GB=x' >>"$W/node.conf"
run "$RD" check
[ $? -eq 2 ] && grep -q 'DOCKER_GB must be a whole number' "$W/err" && ok "a malformed DOCKER_GB is refused" || bad "DOCKER_GB=x: $(cat "$W/err")"
conf
echo 'DOCKER_GB=0' >>"$W/node.conf"
run "$RD" check
[ $? -eq 2 ] && grep -q "DOCKER_GB must be at least 1" "$W/err" && ok "a DOCKER_GB of 0 is refused" || bad "DOCKER_GB=0: $(cat "$W/err")"
conf
run "$RD" apply
# A slot-node process that started before the group change has no group docker, and neither has
# a job it starts: it restarts when no job runs.
echo "wheel adm" >"$RD/.state/procgroups/slot-node.service"
echo 1 >"$RD/.state/jobs"
run "$RD" check
st=$?
[ $st -eq 1 ] && grep -qx "drift: running: slot-node.service: the process started before $me was in group docker" "$W/out" && ok "check names a slot-node that started before the group change" || bad "old slot-node: exit $st $(cat "$W/out")"
: >"$RD/.calls"
run "$RD" apply
grep -qx 'systemctl restart slot-node.service' "$RD/.calls" && bad "slot-node restarted for the group with a job running" || ok "the restart for the group waits while a job runs"
grep -q '^pending: running: slot-node .*group docker' "$W/err" && ok "the restart for the group is a pending line" || bad "no pending line: $(cat "$W/err")"
echo 0 >"$RD/.state/jobs"
: >"$RD/.calls"
run "$RD" apply
grep -qx 'systemctl restart slot-node.service' "$RD/.calls" && ok "with no job running slot-node restarts and gets the group" || bad "no restart: $(cat "$RD/.calls")"
run "$RD" check
[ $? -eq 0 ] && ok "after the restart for the group: 0 drift" || bad "after the group restart: $(cat "$W/out")"
# A user taken out of the group is put back.
: >"$RD/.state/groups"
run "$RD" check
grep -qx "drift: docker: $me is not in group docker" "$W/out" && ok "check names a build user taken out of group docker" || bad "group removed: $(cat "$W/out")"
run "$RD" apply
run "$RD" check
[ $? -eq 0 ] && ok "apply puts the build user back: 0 drift" || bad "after the group is back: $(cat "$W/out")"
# The rules are read from the firewall: the unit stays active when its chain was flushed. They
# are put back without a restart of the unit, which would restart docker and end every container.
rm -f "$RD/.state/imds-containers"
run "$RD" check
st=$?
[ $st -eq 1 ] && grep -qx 'drift: docker: slot-imds-containers.service is active and its reject rules are not in DOCKER-USER' "$W/out" && ok "check names reject rules that are gone while their unit is active" || bad "flushed rules: exit $st $(cat "$W/out")"
: >"$RD/.calls"
run "$RD" apply
grep -q '^iptables -w -I DOCKER-USER 1 -d 169.254.169.254/32 -j REJECT$' "$RD/.calls" && ok "apply puts the reject rule back" || bad "rule not put back: $(cat "$RD/.calls")"
grep -q 'restart slot-imds-containers\|restart docker' "$RD/.calls" && bad "putting the rule back restarts docker: $(cat "$RD/.calls")" || ok "putting the rule back restarts neither the unit nor docker"
run "$RD" check
[ $? -eq 0 ] && ok "after the rule is back: 0 drift" || bad "after the rule is back: $(cat "$W/out")"
rm -f "$RD/.state/imds-containers"
: >"$RD/.state-imds-containers-stay-open"
run "$RD" apply
st=$?
[ $st -eq 1 ] && grep -q '^failed: docker: the reject rules for the metadata service are not in DOCKER-USER' "$W/err" && ok "rules that do not come back: failed line, exit 1" || bad "rules stay gone: exit $st $(tail -3 "$W/err")"
rm -f "$RD/.state-imds-containers-stay-open"
run "$RD" apply
# A drop-in that was edited is drift, rewritten with a daemon-reload.
echo 'Requires=other.service' >"$dd"
run "$RD" check
grep -q '^drift: unit:docker.service.d' "$W/out" && ok "check names a changed docker drop-in" || bad "drop-in drift: $(cat "$W/out")"
: >"$RD/.calls"
run "$RD" apply
grep -qx 'Requires=slot-imds-containers.service' "$dd" && grep -qx 'systemctl daemon-reload' "$RD/.calls" && ok "apply rewrites the drop-in and reloads systemd" || bad "drop-in after apply: $(cat "$dd") calls $(cat "$RD/.calls")"
# The prune script with its commands replaced. `du` prints the KB in $W/dk/used. `docker` logs its
# call; `ps -aq` prints $W/dk/containers, `rm` takes its ids out of that file except those in
# $W/dk/stuck, `builder` and `volume` set used to what after1 and after2 hold, a subcommand
# named in $W/dk/failon fails, and with $W/dk/down every call fails. `runuser` is the pool's
# answer: the line in $W/dk/status (none: slot does not answer), replaced by status2 after one call.
prune=$RD/usr/local/libexec/slot-node/docker-prune.sh
mkdir -p "$W/dk/bin"
cat >"$W/dk/bin/du" <<EOF
#!/bin/sh
[ "\$1" = -skx ] || exit 1
printf '%s\t/var/lib/docker\n' "\$(cat "$W/dk/used")"
EOF
cat >"$W/dk/bin/docker" <<EOF
#!/bin/sh
echo "\$*" >>"$W/dk/calls"
[ -e "$W/dk/down" ] && { echo "Cannot connect to the Docker daemon" >&2; exit 1; }
[ "\$1" = "\$(cat "$W/dk/failon" 2>/dev/null)" ] && exit 1
case "\$1 \$2" in
  'ps -aq') cat "$W/dk/containers" 2>/dev/null ;;
  'rm -fv')
    shift 2
    for c in "\$@"; do
      grep -qx "\$c" "$W/dk/stuck" 2>/dev/null && continue
      grep -vx "\$c" "$W/dk/containers" >"$W/dk/containers.new"
      mv "$W/dk/containers.new" "$W/dk/containers"
    done
    ;;
  'builder prune') [ -f "$W/dk/after1" ] && cp "$W/dk/after1" "$W/dk/used" ;;
  'volume prune') [ -f "$W/dk/after2" ] && cp "$W/dk/after2" "$W/dk/used" ;;
esac
exit 0
EOF
cat >"$W/dk/bin/runuser" <<EOF
#!/bin/sh
echo "\$*" >"$W/dk/runuser.args"
[ -f "$W/dk/status" ] || exit 1
cat "$W/dk/status"
[ -f "$W/dk/status2" ] && mv "$W/dk/status2" "$W/dk/status"
exit 0
EOF
chmod 755 "$W/dk/bin/du" "$W/dk/bin/docker" "$W/dk/bin/runuser"
# dk <KB used> <jobs in the pool, or none> [<containers>]
dk() {
  rm -f "$W/dk/status" "$W/dk/containers"
  : >"$W/dk/calls"
  echo "$1" >"$W/dk/used"
  [ "$2" = none ] || echo "slots 3 of 64 free, $2 running, 0 queued" >"$W/dk/status"
  [ -z "${3:-}" ] || printf '%s\n' $3 >"$W/dk/containers"
  PATH=$W/dk/bin:$PATH sh "$prune" 10 >"$W/out" 2>"$W/err"
}
dkreset() { rm -f "$W/dk/after1" "$W/dk/after2" "$W/dk/status2" "$W/dk/stuck" "$W/dk/failon" "$W/dk/down"; }
dkreset
dk 10485760 0
st=$?
[ $st -eq 0 ] && [ "$(cat "$W/dk/calls")" = 'ps -aq' ] && [ ! -s "$W/out" ] && [ ! -s "$W/err" ] && ok "prune at the cap with no container: nothing is removed and nothing printed" || bad "prune at the cap: exit $st calls $(cat "$W/dk/calls") out $(cat "$W/out" "$W/err")"
grep -qx -- "-u $me -- env XDG_RUNTIME_DIR=/run/user/$uid /srv/slot/bin/slot status" "$W/dk/runuser.args" && ok "prune asks the build user's pool for its jobs" || bad "pool question: $(cat "$W/dk/runuser.args")"
dk 1024 0 'c1 c2'
st=$?
[ $st -eq 0 ] && grep -qx 'rm -fv c1 c2' "$W/dk/calls" && grep -qx 'removed containers left by jobs that ended: 2' "$W/out" && ok "prune with no job in the pool removes every container, also under the cap" || bad "leftover containers: exit $st $(cat "$W/dk/calls" "$W/out" "$W/err")"
grep -q 'prune' "$W/dk/calls" && bad "prune under the cap removed more than containers: $(cat "$W/dk/calls")" || ok "prune under the cap leaves images, volumes and the build cache"
dk 1024 2 'c1 c2'
st=$?
[ $st -eq 0 ] && [ "$(cat "$W/dk/calls")" = 'ps -aq' ] && ok "prune while the pool has a job removes no container" || bad "containers of a running job: exit $st $(cat "$W/dk/calls")"
dk 1024 none 'c1'
st=$?
[ $st -eq 0 ] && [ "$(cat "$W/dk/calls")" = 'ps -aq' ] && ok "prune when slot does not answer removes no container" || bad "no answer from slot: exit $st $(cat "$W/dk/calls")"
echo c2 >"$W/dk/stuck"
dk 1024 0 'c1 c2'
st=$?
[ $st -eq 1 ] && grep -qx 'container c2 could not be removed' "$W/err" && grep -qx 'removed containers left by jobs that ended: 1' "$W/out" && ok "a container that stays is named, exit 1" || bad "stuck container: exit $st $(cat "$W/out" "$W/err")"
dkreset
echo 5242880 >"$W/dk/after1"
dk 12582912 1 'c1'
st=$?
printf 'ps -aq\ncontainer prune -f --filter until=1h\nbuilder prune -af\n' >"$W/dk/want"
[ $st -eq 0 ] && cmp -s "$W/dk/want" "$W/dk/calls" && ok "prune over the cap with a job in the pool: old stopped containers and the build cache go, nothing else" || bad "prune with a job: exit $st $(cat "$W/dk/calls")"
grep -qx 'docker held 12288 MB, cap 10 GB: pruned to 5120 MB' "$W/out" && ok "prune prints what it held and what is left" || bad "prune line: $(cat "$W/out" "$W/err")"
echo 11534336 >"$W/dk/after1"
dk 12582912 1
st=$?
[ $st -eq 1 ] && cmp -s "$W/dk/want" "$W/dk/calls" && grep -qx 'docker holds 11264 MB, cap 10 GB: images and volumes stay while the pool has jobs (jobs: 1)' "$W/err" && [ "$(wc -l <"$W/err")" -eq 1 ] && ok "prune still over the cap with a job in the pool: images and volumes stay, one line, exit 1" || bad "over the cap with a job: exit $st $(cat "$W/dk/calls" "$W/err")"
dk 12582912 none
st=$?
[ $st -eq 1 ] && cmp -s "$W/dk/want" "$W/dk/calls" && grep -q '(jobs: unknown)$' "$W/err" && ok "prune over the cap when slot does not answer: images and volumes stay" || bad "over the cap, no answer: exit $st $(cat "$W/dk/calls" "$W/err")"
echo 2097152 >"$W/dk/after2"
dk 12582912 0
st=$?
printf 'image prune -af\nvolume prune -af\n' >>"$W/dk/want"
[ $st -eq 0 ] && cmp -s "$W/dk/want" "$W/dk/calls" && ok "prune over the cap with no job in the pool: every unused image and volume goes" || bad "prune with no job: exit $st $(cat "$W/dk/calls" "$W/err")"
# A job that came in while the first calls ran keeps its images.
dk_late() {
  echo 'slots 3 of 64 free, 1 running, 0 queued' >"$W/dk/status2"
  dk 12582912 0
}
dk_late
st=$?
[ $st -eq 1 ] && ! grep -q '^image prune\|^volume prune' "$W/dk/calls" && grep -q '(jobs: a new one)$' "$W/err" && ok "a job that came in during the prune keeps images and volumes" || bad "late job: exit $st $(cat "$W/dk/calls" "$W/err")"
dkreset
echo 11534336 >"$W/dk/after2"
dk 12582912 0
st=$?
[ $st -eq 1 ] && grep -qx 'docker holds 11264 MB with no job in the pool and everything unused removed, cap 10 GB' "$W/err" && ok "prune that cannot reach the cap on an idle node: exit 1 and one line" || bad "over after everything: exit $st $(cat "$W/out" "$W/err")"
dkreset
: >"$W/dk/down"
dk 12582912 0 'c1'
st=$?
[ $st -eq 1 ] && [ "$(cat "$W/dk/calls")" = 'ps -aq' ] && grep -qx 'docker does not answer: nothing is pruned' "$W/err" && ok "prune with dockerd down: one call, one line, exit 1" || bad "dockerd down: exit $st $(cat "$W/dk/calls" "$W/err")"
dkreset
echo builder >"$W/dk/failon"
echo 5242880 >"$W/dk/after2"
dk 12582912 0
st=$?
[ $st -eq 1 ] && grep -qx 'docker builder prune -af failed' "$W/err" && ok "a docker call that fails is named, exit 1 also under the cap afterwards" || bad "failed docker call: exit $st $(cat "$W/out" "$W/err")"
dkreset
grep -q 'system prune\|docker kill\|docker stop\|rm -rf' "$prune" && bad "the prune script has a removal beside rm of the listed containers and the prunes" || ok "the prune script removes through docker rm of the listed containers and the prunes only"

# 6p. role.sh: a node's own IAM role and instance profile with one policy that allows the Systems
# Manager agent its channels and nothing a job could use. `aws` is replaced: it logs its call and
# keeps the account in files under $W/aws (policy, role, attached, inline, profile, assoc).
RS=$here/role.sh
A=$W/aws
mkdir -p "$A/bin"
cat >"$A/bin/aws" <<EOF
#!/bin/sh
A=$A
EOF
cat >>"$A/bin/aws" <<'EOF'
echo "$*" >>"$A/calls"
arg() {
  want=$1
  shift
  while [ $# -gt 1 ]; do
    [ "$1" = "$want" ] && {
      echo "$2"
      return 0
    }
    shift
  done
}
[ "$1 $2" = "$(cat "$A/failon" 2>/dev/null)" ] && {
  echo "An error occurred (Stub) when calling $2" >&2
  exit 254
}
case "$1 $2" in
  'sts get-caller-identity') echo arn:aws:iam::111122223333:user/operator ;;
  'iam get-policy') [ -f "$A/policy" ] && echo v3 || exit 254 ;;
  'iam get-policy-version') cat "$A/policy" ;;
  'iam create-policy' | 'iam create-policy-version') arg --policy-document "$@" >"$A/policy" ;;
  'iam get-role') [ -f "$A/role" ] && cat "$A/role" || exit 254 ;;
  'iam create-role') arg --assume-role-policy-document "$@" >"$A/role" ;;
  'iam update-assume-role-policy') arg --policy-document "$@" >"$A/role" ;;
  'iam list-attached-role-policies') [ -f "$A/role" ] || exit 254; tr '\n' '\t' <"$A/attached" 2>/dev/null ;;
  'iam attach-role-policy') arg --policy-arn "$@" >>"$A/attached" ;;
  'iam detach-role-policy')
    grep -vx "$(arg --policy-arn "$@")" "$A/attached" >"$A/attached.new"
    mv "$A/attached.new" "$A/attached"
    ;;
  'iam list-role-policies') [ -f "$A/role" ] || exit 254; cat "$A/inline" 2>/dev/null || echo None ;;
  'iam get-instance-profile')
    [ -f "$A/profile" ] || exit 254
    case "$*" in *Roles*) cat "$A/profile" ;; *) arg --instance-profile-name "$@" ;; esac
    ;;
  'iam create-instance-profile') : >"$A/profile" ;;
  'iam add-role-to-instance-profile') arg --role-name "$@" >"$A/profile" ;;
  'ec2 describe-iam-instance-profile-associations') cat "$A/assoc" 2>/dev/null ;;
  'ec2 associate-iam-instance-profile' | 'ec2 replace-iam-instance-profile-association')
    if [ -f "$A/not-yet" ]; then
      rm -f "$A/not-yet"
      echo 'An error occurred (InvalidParameterValue): Invalid IAM Instance Profile name' >&2
      exit 254
    fi
    p=$(arg --iam-instance-profile "$@")
    printf 'iip-assoc-new\tarn:aws:iam::111122223333:instance-profile/%s\n' "${p#Name=}" >"$A/assoc"
    ;;
  'ec2 describe-instances') cat "$A/metadata" 2>/dev/null || printf 'required\t1\n' ;;
  'ec2 modify-instance-metadata-options') printf '%s\t%s\n' "$(arg --http-tokens "$@")" "$(arg --http-put-response-hop-limit "$@")" >"$A/metadata" ;;
  *)
    echo "aws stub: no answer for $*" >&2
    exit 2
    ;;
esac
EOF
chmod 755 "$A/bin/aws"
role() {
  : >"$A/calls"
  AWS=$A/bin/aws ROLE_WAIT=0 ROLE_TRIES=3 sh "$RS" "$@" >"$W/out" 2>"$W/err"
}
# The policy: the agent's heartbeat and its two message services, and no action that reads data.
sh "$RS" policy >"$W/policy.json"
grep -o '"[a-z0-9]*:[A-Za-z]*"' "$W/policy.json" | tr -d '"' | LC_ALL=C sort >"$W/actions"
printf '%s\n' ec2messages:AcknowledgeMessage ec2messages:DeleteMessage ec2messages:FailMessage ec2messages:GetEndpoint ec2messages:GetMessages ec2messages:SendReply ssm:UpdateInstanceInformation ssmmessages:CreateControlChannel ssmmessages:CreateDataChannel ssmmessages:OpenControlChannel ssmmessages:OpenDataChannel >"$W/actions.want"
cmp -s "$W/actions" "$W/actions.want" && ok "the node policy allows the agent's 11 actions and no other" || bad "policy actions: $(tr '\n' ' ' <"$W/actions")"
grep -qi 'GetParameter\|GetDocument\|"\*"[^}]*Action\|Action[^]]*\*"' "$W/policy.json" && bad "the node policy reads parameters or documents, or has a wildcard action" || ok "the node policy has no GetParameter, no GetDocument and no wildcard action"
grep -q '"Effect": "Deny"\|NotAction\|NotResource' "$W/policy.json" && bad "the node policy has a Deny or a Not element" || ok "the node policy is a plain allow list"
sh "$RS" trust | tr -d ' \n' | grep -q '"Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"' && ok "the role is assumed by ec2 only" || bad "trust: $(sh "$RS" trust | tr -d '\n')"
# plan prints the commands of a first setup and calls nothing.
role plan node-1 i-0123456789abcdef0
st=$?
[ $st -eq 0 ] && [ ! -s "$A/calls" ] && ok "plan calls no aws command" || bad "plan: exit $st calls $(cat "$A/calls")"
for line in "aws iam create-policy --policy-name slot-node-agent " "aws iam create-role --role-name slot-node-node-1 " 'aws iam attach-role-policy --role-name slot-node-node-1 --policy-arn "arn:aws:iam::$ACCOUNT:policy/slot-node-agent"' "aws iam create-instance-profile --instance-profile-name slot-node-node-1" "aws iam add-role-to-instance-profile --instance-profile-name slot-node-node-1 --role-name slot-node-node-1" 'aws ec2 replace-iam-instance-profile-association --association-id "$ASSOCIATION" --iam-instance-profile Name=slot-node-node-1' 'aws ec2 modify-instance-metadata-options --instance-id i-0123456789abcdef0 --http-tokens required --http-put-response-hop-limit 1'; do
  grep -qF -- "$line" "$W/out" || bad "plan lacks: $line"
done
[ "$(grep -c '^aws \|^[A-Z]*=\$(aws ' "$W/out")" -eq 9 ] && ok "plan prints 9 commands: policy, role, attach, profile, role into profile, association, metadata options" || bad "plan: $(cat "$W/out")"
grep -F -- "--policy-document '$(tr -d ' \n' <"$W/policy.json")'" "$W/out" >/dev/null && ok "plan carries the policy document on one line" || bad "plan policy document: $(grep create-policy "$W/out")"
for badarg in 'no/de i-0123456789abcdef0' 'node-1 0123' 'node-1 i-012z' '-n i-0123456789abcdef0'; do
  role plan $badarg
  [ $? -eq 2 ] && ok "plan refuses the arguments '$badarg'" || bad "plan accepts '$badarg': $(cat "$W/out" | head -2)"
done
# An account with nothing of it, and the instance on a shared profile.
rm -f "$A/policy" "$A/role" "$A/attached" "$A/inline" "$A/profile" "$A/failon" "$A/not-yet"
printf 'iip-assoc-old\tarn:aws:iam::111122223333:instance-profile/SharedManagedRole\n' >"$A/assoc"
role check node-1 i-0123456789abcdef0
st=$?
[ $st -eq 1 ] && [ "$(grep -c '^drift: role: ' "$W/out")" -eq 6 ] && ok "check on a bare account: 6 drift lines, exit 1" || bad "bare account: exit $st $(cat "$W/out" "$W/err")"
grep -qx "drift: role: instance i-0123456789abcdef0 uses the instance profile 'SharedManagedRole', not slot-node-node-1" "$W/out" && ok "check names the profile the instance uses" || bad "profile drift: $(grep instance "$W/out")"
grep -q 'create-\|attach-\|add-role\|replace-\|associate-\|detach-\|update-\|delete-\|put-\|modify-' "$A/calls" && bad "check changes the account: $(grep 'create-\|attach-\|add-role\|replace-\|associate-' "$A/calls")" || ok "check only reads"
: >"$A/not-yet"
role apply node-1 i-0123456789abcdef0
st=$?
[ $st -eq 0 ] && grep -qx '0 drift' "$W/out" && ok "apply on a bare account ends with 0 drift" || bad "apply: exit $st $(cat "$W/out" "$W/err")"
[ "$(tr -d ' \n' <"$A/policy")" = "$(tr -d ' \n' <"$W/policy.json")" ] && ok "apply creates the policy with the document of role.sh policy" || bad "created policy: $(cat "$A/policy")"
[ "$(cat "$A/attached")" = arn:aws:iam::111122223333:policy/slot-node-agent ] && [ "$(cat "$A/profile")" = slot-node-node-1 ] && ok "apply attaches the one policy and puts the role into its profile" || bad "attached: $(cat "$A/attached") profile: $(cat "$A/profile")"
grep -q 'instance-profile/slot-node-node-1$' "$A/assoc" && grep -q '^ec2 replace-iam-instance-profile-association --association-id iip-assoc-old --iam-instance-profile Name=slot-node-node-1$' "$A/calls" && ok "apply replaces the instance's profile association" || bad "association: $(cat "$A/assoc") $(grep ec2 "$A/calls")"
[ "$(grep -c '^ec2 replace-iam-instance-profile-association' "$A/calls")" -eq 2 ] && ok "apply tries again while EC2 does not know the new profile yet" || bad "retries: $(grep -c '^ec2 replace' "$A/calls")"
grep -q '^note: credentials the old role gave out stay valid until they expire' "$W/out" && ok "apply says that the old role's credentials outlive the change" || bad "no note: $(cat "$W/out")"
role apply node-1 i-0123456789abcdef0
st=$?
[ $st -eq 0 ] && ! grep -q '^changed' "$W/out" && ! grep -q 'create-\|attach-\|add-role\|replace-\|associate-' "$A/calls" && ok "a second apply changes nothing" || bad "second apply: exit $st $(cat "$W/out")"
# Each difference is named and closed.
echo arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore >>"$A/attached"
role check node-1 i-0123456789abcdef0
grep -qx 'drift: role: role slot-node-node-1 has another policy attached: arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore' "$W/out" && ok "check names a second policy on the role" || bad "second policy: $(cat "$W/out")"
role apply node-1 i-0123456789abcdef0
[ "$(cat "$A/attached")" = arn:aws:iam::111122223333:policy/slot-node-agent ] && ok "apply detaches the second policy" || bad "after detach: $(cat "$A/attached")"
sed 's/ssm:UpdateInstanceInformation/ssm:GetParameter/' "$W/policy.json" >"$A/policy"
role check node-1 i-0123456789abcdef0
grep -q '^drift: role: policy slot-node-agent version v3 is not the document' "$W/out" && ok "check names a policy whose document changed" || bad "changed policy: $(cat "$W/out")"
role apply node-1 i-0123456789abcdef0
grep -q '^iam create-policy-version .* --set-as-default$' "$A/calls" && ! grep -q GetParameter "$A/policy" && ok "apply writes a new default version of the policy" || bad "policy version: $(grep policy-version "$A/calls")"
echo extra >"$A/inline"
role apply node-1 i-0123456789abcdef0
st=$?
[ $st -eq 1 ] && grep -q '^failed: role: inline policy extra of role slot-node-node-1 stays' "$W/err" && ! grep -q delete-role-policy "$A/calls" && ok "an inline policy is named and not deleted: failed line, exit 1" || bad "inline policy: exit $st $(cat "$W/err")"
rm -f "$A/inline"
: >"$A/assoc"
role check node-1 i-0123456789abcdef0
grep -qx "drift: role: instance i-0123456789abcdef0 uses the instance profile 'none', not slot-node-node-1" "$W/out" && ok "check names an instance without a profile" || bad "no profile: $(cat "$W/out")"
role apply node-1 i-0123456789abcdef0
grep -q '^ec2 associate-iam-instance-profile --instance-id i-0123456789abcdef0 --iam-instance-profile Name=slot-node-node-1$' "$A/calls" && ok "apply associates the profile with an instance that has none" || bad "associate: $(grep ec2 "$A/calls")"
# The metadata service answers a plain GET (IMDSv1) or lets its answer travel 2 hops.
printf 'optional\t2\n' >"$A/metadata"
role check node-1 i-0123456789abcdef0
st=$?
[ $st -eq 1 ] && grep -qx "drift: role: instance i-0123456789abcdef0: the metadata service has tokens and hop limit 'optional 2', not 'required 1'" "$W/out" && ok "check names a metadata service that answers without a token" || bad "metadata options: exit $st $(cat "$W/out")"
role apply node-1 i-0123456789abcdef0
st=$?
[ $st -eq 0 ] && grep -q '^ec2 modify-instance-metadata-options --instance-id i-0123456789abcdef0 --http-tokens required --http-put-response-hop-limit 1$' "$A/calls" && [ "$(cat "$A/metadata")" = "$(printf 'required\t1')" ] && ok "apply sets tokens required and hop limit 1" || bad "metadata apply: exit $st $(grep modify "$A/calls") $(cat "$W/out" "$W/err")"
printf 'required\t2\n' >"$A/metadata"
role check node-1 i-0123456789abcdef0
grep -q "tokens and hop limit 'required 2'" "$W/out" && ok "check names a hop limit of 2" || bad "hop limit 2: $(cat "$W/out")"
rm -f "$A/metadata"
# A failed step keeps the instance on the profile it has.
rm -f "$A/policy" "$A/role" "$A/attached" "$A/profile"
printf 'iip-assoc-old\tarn:aws:iam::111122223333:instance-profile/SharedManagedRole\n' >"$A/assoc"
echo 'iam create-role' >"$A/failon"
role apply node-1 i-0123456789abcdef0
st=$?
[ $st -eq 1 ] && grep -q '^failed: role: aws iam create-role' "$W/err" && grep -q SharedManagedRole "$A/assoc" && ! grep -q '^ec2 replace\|^ec2 associate' "$A/calls" && ok "a failed step: exit 1, and the instance keeps its profile" || bad "failed step: exit $st $(cat "$W/err") $(cat "$A/assoc")"
rm -f "$A/failon"
# The whole document is compared: an action with a wildcard or another Resource is a difference.
[ "$(tr -d ' \n' <"$W/policy.json")" = '{"Version":"2012-10-17","Statement":[{"Sid":"AgentHeartbeat","Effect":"Allow","Action":"ssm:UpdateInstanceInformation","Resource":"*"},{"Sid":"AgentChannels","Effect":"Allow","Action":["ssmmessages:CreateControlChannel","ssmmessages:CreateDataChannel","ssmmessages:OpenControlChannel","ssmmessages:OpenDataChannel"],"Resource":"*"},{"Sid":"AgentChannelsOlderAgent","Effect":"Allow","Action":["ec2messages:AcknowledgeMessage","ec2messages:DeleteMessage","ec2messages:FailMessage","ec2messages:GetEndpoint","ec2messages:GetMessages","ec2messages:SendReply"],"Resource":"*"}]}' ] && ok "the node policy is the reviewed document, byte for byte without blanks" || bad "policy document: $(tr -d ' \n' <"$W/policy.json")"
n54=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
role plan $n54 i-0123456789abcdef0
[ $? -eq 0 ] && ok "a NAME of 54 characters is accepted (a role name of 64)" || bad "54 characters: $(cat "$W/err")"
role plan ${n54}a i-0123456789abcdef0
[ $? -eq 2 ] && ok "a NAME of 55 characters is refused" || bad "55 characters accepted"
role plan node-1 i-0123456789abcdef0
grep -q '^# An instance without a profile .*aws ec2 associate-iam-instance-profile --instance-id i-0123456789abcdef0 --iam-instance-profile Name=slot-node-node-1$' "$W/out" && ok "plan names the command for an instance without a profile" || bad "plan without a profile: $(grep associate "$W/out")"
# A whole account again.
rm -f "$A/policy" "$A/role" "$A/attached" "$A/inline" "$A/profile" "$A/failon" "$A/not-yet" "$A/metadata"
printf 'iip-assoc-old\tarn:aws:iam::111122223333:instance-profile/SharedManagedRole\n' >"$A/assoc"
role apply node-1 i-0123456789abcdef0
[ $? -eq 0 ] || bad "apply before the next cases: $(cat "$W/out" "$W/err")"
role check node-1 i-0123456789abcdef0
grep -q '^ec2 describe-iam-instance-profile-associations .*Name=state,Values=associating,associated ' "$A/calls" && ok "an association that is still associating counts" || bad "association filter: $(grep describe-iam "$A/calls")"
# The instance is read first: another region or account stops before anything is made.
rm -f "$A/policy"
echo 'ec2 describe-instances' >"$A/failon"
role apply node-1 i-0123456789abcdef0
st=$?
[ $st -eq 2 ] && grep -q 'describe-instances does not show i-0123456789abcdef0' "$W/err" && ! grep -q 'create-\|attach-\|replace-\|associate-\|modify-' "$A/calls" && ok "an instance that cannot be read: exit 2 before any change" || bad "unreadable instance: exit $st $(cat "$W/err") $(cat "$A/calls")"
rm -f "$A/failon"
role apply node-1 i-0123456789abcdef0
# A role that others may assume.
echo '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"*"},"Action":"sts:AssumeRole"}]}' >"$A/role"
role check node-1 i-0123456789abcdef0
st=$?
[ $st -eq 1 ] && grep -qx 'drift: role: role slot-node-node-1 is not assumed by ec2.amazonaws.com alone' "$W/out" && ok "check names a role with another trust document" || bad "trust: exit $st $(cat "$W/out")"
role apply node-1 i-0123456789abcdef0
grep -q '^iam update-assume-role-policy --role-name slot-node-node-1 ' "$A/calls" && grep -q '"Service":"ec2.amazonaws.com"' "$A/role" && ok "apply writes the trust document back" || bad "trust apply: $(cat "$A/calls" | grep assume)"
# An inline policy, and a list of them that cannot be read.
echo extra >"$A/inline"
role check node-1 i-0123456789abcdef0
st=$?
[ $st -eq 1 ] && grep -qx 'drift: role: role slot-node-node-1 has an inline policy: extra' "$W/out" && ok "check names an inline policy, exit 1" || bad "inline in check: exit $st $(cat "$W/out")"
echo 'iam list-role-policies' >"$A/failon"
role check node-1 i-0123456789abcdef0
st=$?
[ $st -eq 1 ] && grep -qx 'drift: role: the inline policies of role slot-node-node-1 could not be read' "$W/out" && ok "inline policies that cannot be read are drift, not an empty list" || bad "unreadable inline policies: exit $st $(cat "$W/out")"
rm -f "$A/failon" "$A/inline"
# A role that carries only another policy: the node's policy goes on before the other comes off.
echo arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore >"$A/attached"
role apply node-1 i-0123456789abcdef0
st=$?
at=$(grep -n '^iam attach-role-policy ' "$A/calls" | head -n 1 | cut -d: -f1)
dt=$(grep -n '^iam detach-role-policy ' "$A/calls" | head -n 1 | cut -d: -f1)
[ $st -eq 0 ] && [ -n "$at" ] && [ -n "$dt" ] && [ "$at" -lt "$dt" ] && ok "apply attaches the node's policy before it detaches another" || bad "attach before detach: exit $st attach at ${at:-none}, detach at ${dt:-none}"
rm -f "$A/policy"
echo arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore >"$A/attached"
echo 'iam create-policy' >"$A/failon"
role apply node-1 i-0123456789abcdef0
st=$?
[ $st -eq 1 ] && ! grep -q '^iam detach-role-policy' "$A/calls" && grep -q AmazonSSMManagedInstanceCore "$A/attached" && grep -q '^failed: role: policy arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore stays on role slot-node-node-1' "$W/err" && ok "when the node's policy cannot be made the role keeps the policy it has" || bad "detach after a failure: exit $st $(grep detach "$A/calls") $(cat "$W/err")"
rm -f "$A/failon"
role apply node-1 i-0123456789abcdef0
# The profile holds another role.
echo other-role >"$A/profile"
role check node-1 i-0123456789abcdef0
grep -qx 'drift: role: instance profile slot-node-node-1 holds the role other-role' "$W/out" && ok "check names a profile that holds another role" || bad "other role: $(cat "$W/out")"
role apply node-1 i-0123456789abcdef0
st=$?
[ $st -eq 1 ] && grep -q '^failed: role: instance profile slot-node-node-1 holds another role: aws iam remove-role-from-instance-profile ' "$W/err" && [ "$(cat "$A/profile")" = other-role ] && ok "apply leaves another role in the profile: failed line with the command, exit 1" || bad "other role apply: exit $st $(cat "$W/err")"
echo slot-node-node-1 >"$A/profile"
# EC2 never takes the profile.
printf 'iip-assoc-old\tarn:aws:iam::111122223333:instance-profile/SharedManagedRole\n' >"$A/assoc"
echo 'ec2 replace-iam-instance-profile-association' >"$A/failon"
role apply node-1 i-0123456789abcdef0
st=$?
[ $st -eq 1 ] && [ "$(grep -c '^ec2 replace-iam-instance-profile-association' "$A/calls")" -eq 3 ] && grep -q '^failed: role: instance i-0123456789abcdef0 did not take the instance profile slot-node-node-1: ' "$W/err" && ! grep -q '^changed: role: instance' "$W/out" && ok "an association that fails every time: the tries end, failed line, exit 1" || bad "tries used up: exit $st $(grep -c '^ec2 replace' "$A/calls") $(cat "$W/out" "$W/err")"
rm -f "$A/failon"

# 6q. Jobs get docker only on an instance that carries the node's own profile (or none) and whose
# metadata service refuses a request without a token. provision.sh reads both from the metadata
# service and calls no AWS API. `check --canary` starts one container that tries the role.
conf
RR=$W/r12
mkdir -p "$RR/.state"
echo 0 >"$RR/.state/jobs"
# prof <profile | - | code:<status>> [<role>]: what the metadata service says about the instance
# profile of the root $RR: its iam/info document and the role it gives out, no profile, or a
# status without a document.
prof() {
  rm -f "$RR/.state/iam-info" "$RR/.state/iam-info-code" "$RR/.state/instance-role"
  case $1 in
    -) ;;
    code:*) echo "${1#code:}" >"$RR/.state/iam-info-code" ;;
    *)
      printf '{\n  "Code" : "Success",\n  "LastUpdated" : "2026-01-01T00:00:00Z",\n  "InstanceProfileArn" : "arn:aws:iam::111122223333:instance-profile/%s",\n  "InstanceProfileId" : "AIPAEXAMPLEEXAMPLE"\n}\n' "$1" >"$RR/.state/iam-info"
      echo "${2-$1}" >"$RR/.state/instance-role"
      ;;
  esac
}
# norole: check ran to its end and has no line of the role part.
norole() { grep -q ' drift$' "$W/out" && ! grep -q '^drift: role:' "$W/out"; }
prof SharedManagedRole
run "$RR" check
grep -qx "drift: role: the instance profile is 'SharedManagedRole', not slot-node-node-1 (role.sh apply makes it)" "$W/out" && ok "check names an instance profile that is not the node's own" || bad "role drift: $(grep 'role' "$W/out")"
grep -q '^drift: docker: ' "$W/out" && bad "with another profile the missing group is a second line: $(grep '^drift: docker' "$W/out")" || ok "with another profile the missing group is no line of its own"
run "$RR" apply
st=$?
[ $st -eq 1 ] && grep -q "^failed: role: the instance profile is 'SharedManagedRole', not slot-node-node-1 (role.sh apply makes it): $me gets no group docker$" "$W/err" && ok "apply with another profile: failed line, exit 1" || bad "apply with another profile: exit $st $(grep role "$W/err")"
grep -q docker "$RR/.state/groups" 2>/dev/null && bad "the build user got group docker under another profile" || ok "apply with another profile leaves the build user out of group docker"
grep -qw docker "$RR/.state/procgroups/slot-node.service" 2>/dev/null && bad "slot-node has group docker under another profile" || ok "slot-node starts without group docker"
grep -qx docker "$RR/.state/packages" && [ -f "$RR/.state/active/slot-node.service" ] && ok "every other part of the node is made all the same" || bad "other parts: $(ls "$RR/.state/active" | tr '\n' ' ')"
prof slot-node-node-1
run "$RR" check
grep -qx "drift: docker: $me is not in group docker" "$W/out" && norole && ok "with the node's own profile the missing group is the drift" || bad "own profile: $(cat "$W/out")"
run "$RR" apply
grep -qx "changed: docker: $me added to group docker" "$W/out" && ok "with the node's own profile apply adds the group" || bad "own profile apply: $(cat "$W/out" "$W/err" | cut -c1-300)"
run "$RR" apply
run "$RR" check
st=$?
[ $st -eq 0 ] && ok "the node's own profile: 0 drift" || bad "own profile: exit $st $(cat "$W/out")"
# The instance goes back to a shared profile while the build user has the group and slot-node
# runs without it (it started before the group): apply does not restart it into docker.
echo "wheel adm" >"$RR/.state/procgroups/slot-node.service"
prof SharedManagedRole
run "$RR" check
st=$?
[ $st -eq 1 ] && [ "$(grep -c '^drift: ' "$W/out")" -eq 1 ] && grep -q "^drift: role: the instance profile is 'SharedManagedRole'" "$W/out" && ok "a profile changed afterwards is one drift line, and no restart for the group is asked" || bad "profile changed: exit $st $(cat "$W/out")"
: >"$RR/.calls"
run "$RR" apply
st=$?
[ $st -eq 1 ] && grep -q "^failed: role: .*: $me is in group docker, and a container of a job reaches the credentials of this role (gpasswd -d $me docker and a restart of slot-node.service take the group away)$" "$W/err" && ok "apply names a build user that has docker under another profile, with the command, exit 1" || bad "docker under another profile: exit $st $(cat "$W/err")"
grep -q 'systemctl restart slot-node.service' "$RR/.calls" && bad "apply restarted slot-node into group docker under another profile" || ok "under another profile slot-node is not restarted for the group"
[ "$(cat "$RR/.state/procgroups/slot-node.service")" = "wheel adm" ] && ok "the jobs of that slot-node stay without docker" || bad "slot-node groups: $(cat "$RR/.state/procgroups/slot-node.service")"
prof slot-node-node-1
: >"$RR/.calls"
run "$RR" apply
grep -qx 'systemctl restart slot-node.service' "$RR/.calls" && grep -qw docker "$RR/.state/procgroups/slot-node.service" && ok "back under the node's own profile slot-node restarts and gets the group" || bad "restart under the own profile: $(cat "$RR/.calls" "$W/out" "$W/err" | cut -c1-300)"
# What the metadata service answers.
prof -
run "$RR" check
[ $? -eq 0 ] && ok "an instance without a profile has no credentials to give: no drift" || bad "no profile: $(cat "$W/out")"
prof code:500
run "$RR" check
grep -qx 'drift: role: the metadata service does not name the instance profile' "$W/out" && ok "no answer about the profile is drift" || bad "no answer: $(cat "$W/out")"
prof slot-node-node-1
echo '<html><body>200</body></html>' >"$RR/.state/iam-info"
run "$RR" check
grep -qx 'drift: role: the metadata service does not name the instance profile' "$W/out" && ok "an answer without a profile name is drift" || bad "html answer: $(cat "$W/out")"
echo '{"Code":"Success","InstanceProfileArn":"arn:aws:iam::111122223333:instance-profile/slot-node-node-1","InstanceProfileId":"AIPAEXAMPLEEXAMPLE"}' >"$RR/.state/iam-info"
run "$RR" check
[ $? -eq 0 ] && ok "the profile name is read from a document on one line" || bad "one line: $(cat "$W/out")"
prof team/build/slot-node-node-1 slot-node-node-1
run "$RR" check
[ $? -eq 0 ] && ok "the profile name is read from an ARN with a path" || bad "path: $(cat "$W/out")"
prof slot-node-node-1 other-role
run "$RR" check
grep -qx "drift: role: the instance profile slot-node-node-1 gives out the role 'other-role', not the role of its name (role.sh check names the difference)" "$W/out" && ok "the node's profile with another role in it is drift" || bad "other role: $(cat "$W/out")"
prof slot-node-node-1 ''
run "$RR" check
grep -q "^drift: role: the instance profile slot-node-node-1 gives out the role 'unknown'" "$W/out" && ok "the node's profile whose role is not named is drift" || bad "no role name: $(cat "$W/out")"
prof none none
run "$RR" check
grep -qx "drift: role: the instance profile is 'none', not slot-node-node-1 (role.sh apply makes it)" "$W/out" && ok "a profile with the name none is a profile" || bad "profile named none: $(cat "$W/out")"
# INSTANCE_PROFILE: another profile as the node's own, and none for a machine outside EC2.
echo 'INSTANCE_PROFILE=none' >>"$W/node.conf"
run "$RR" check
grep -qx "drift: role: the instance profile is 'none', and the config says INSTANCE_PROFILE=none" "$W/out" && ok "INSTANCE_PROFILE=none does not accept a profile with the name none" || bad "none and a profile named none: $(cat "$W/out")"
prof SharedManagedRole
run "$RR" check
grep -qx "drift: role: the instance profile is 'SharedManagedRole', and the config says INSTANCE_PROFILE=none" "$W/out" && ok "INSTANCE_PROFILE=none on an instance that has a profile is drift" || bad "none with a profile: $(grep role "$W/out")"
prof code:000
run "$RR" check
grep -qx 'drift: role: the metadata service does not name the instance profile' "$W/out" && ok "INSTANCE_PROFILE=none on an EC2 instance: no answer is still drift" || bad "none on EC2, no answer: $(cat "$W/out")"
: >"$RR/.state/not-ec2"
echo 000 >"$RR/.state/imds-v1-code"
run "$RR" check
norole && ok "INSTANCE_PROFILE=none on a machine outside EC2: nothing is asked" || bad "outside EC2: $(cat "$W/out" "$W/err")"
rm -f "$RR/.state/not-ec2" "$RR/.state/imds-v1-code"
conf
echo 'INSTANCE_PROFILE=build-nodes' >>"$W/node.conf"
prof build-nodes some-role
run "$RR" check
norole && ok "INSTANCE_PROFILE names another profile as the node's own; its role is not compared" || bad "INSTANCE_PROFILE names the profile: $(cat "$W/out" "$W/err")"
prof slot-node-node-1
run "$RR" check
grep -qx "drift: role: the instance profile is 'slot-node-node-1', not build-nodes (role.sh apply makes it)" "$W/out" && ok "with INSTANCE_PROFILE set the name from NODE_NAME is not accepted" || bad "INSTANCE_PROFILE wins: $(grep role "$W/out")"
conf
grep -v '^NODE_NAME=' "$W/node.conf" >"$W/c2" && mv "$W/c2" "$W/node.conf"
prof SharedManagedRole
run "$RR" check
grep -qx "drift: role: the instance profile is 'SharedManagedRole' and the config names none (NODE_NAME or INSTANCE_PROFILE)" "$W/out" && ok "a config without NODE_NAME and INSTANCE_PROFILE accepts no profile" || bad "no name: $(grep role "$W/out")"
for bad_prof in 'INSTANCE_PROFILE=a b' 'INSTANCE_PROFILE=x;id' 'INSTANCE_PROFILE=$(id)'; do
  conf
  echo "$bad_prof" >>"$W/node.conf"
  run "$RR" check
  [ $? -eq 2 ] && grep -q 'INSTANCE_PROFILE must be the name of an instance profile' "$W/err" && ok "$bad_prof is refused" || bad "$bad_prof is accepted ($(cat "$W/err"))"
done
# A request without a token: answered, refused, or neither. On a root whose build user has no
# group yet, every reason keeps the group away.
conf
RR=$W/r13
mkdir -p "$RR/.state"
echo 0 >"$RR/.state/jobs"
prof slot-node-node-1
echo 200 >"$RR/.state/imds-v1-code"
run "$RR" check
st=$?
[ $st -eq 1 ] && grep -qx 'drift: role: the metadata service answers without a token (role.sh apply sets tokens required)' "$W/out" && ok "check names a metadata service that answers without a token" || bad "IMDSv1: exit $st $(grep role "$W/out")"
run "$RR" apply
st=$?
[ $st -eq 1 ] && grep -q "^failed: role: the metadata service answers without a token .*: $me gets no group docker$" "$W/err" && ! grep -q docker "$RR/.state/groups" 2>/dev/null && ok "an answer without a token: failed line, exit 1, no group docker" || bad "IMDSv1 apply: exit $st $(cat "$W/err") groups $(cat "$RR/.state/groups" 2>&1)"
echo 000 >"$RR/.state/imds-v1-code"
run "$RR" apply
st=$?
[ $st -eq 1 ] && grep -q '^failed: role: the metadata service neither refused nor answered a request without a token: ' "$W/err" && ! grep -q docker "$RR/.state/groups" 2>/dev/null && ok "no answer to a request without a token: failed line, no group docker" || bad "no answer without a token: exit $st $(cat "$W/err")"
echo 401 >"$RR/.state/imds-v1-code"
prof code:000
run "$RR" apply
st=$?
[ $st -eq 1 ] && grep -q '^failed: role: the metadata service does not name the instance profile: ' "$W/err" && ! grep -q docker "$RR/.state/groups" 2>/dev/null && ok "no answer about the profile: failed line, no group docker" || bad "no profile answer apply: exit $st $(cat "$W/err")"
prof slot-node-node-1 other-role
run "$RR" apply
st=$?
[ $st -eq 1 ] && grep -q "^failed: role: the instance profile slot-node-node-1 gives out the role 'other-role'" "$W/err" && ! grep -q docker "$RR/.state/groups" 2>/dev/null && ok "another role in the node's profile: failed line, no group docker" || bad "other role apply: exit $st $(cat "$W/err")"
prof slot-node-node-1
echo 403 >"$RR/.state/imds-v1-code"
run "$RR" apply
st=$?
[ $st -eq 0 ] && grep -qx "$me:docker" "$RR/.state/groups" && ok "a metadata service that is off for requests without a token (403) lets the group in" || bad "403: exit $st $(cat "$W/out" "$W/err" | cut -c1-300)"
rm -f "$RR/.state/imds-v1-code"
grep -v '^ *#' "$P" | grep -q 'aws \(iam\|ec2\|sts\|ssm\) ' && bad "provision.sh calls the aws command" || ok "provision.sh calls no aws command"
# An option a mode does not know is refused.
NODE_TEST=1 NODE_ROOT=$RR sh "$P" check "$W/node.conf" --Canary >"$W/out" 2>"$W/err"
[ $? -eq 2 ] && ok "check with a misspelled --canary is refused" || bad "check --Canary ran: $(cat "$W/out")"
NODE_TEST=1 NODE_ROOT=$RR sh "$P" apply "$W/node.conf" --canary >"$W/out" 2>"$W/err"
[ $? -eq 2 ] && ok "apply --canary is refused" || bad "apply --canary ran: $(cat "$W/out")"
sh "$P" bundle "$W/node.conf" check --open >"$W/out" 2>"$W/err"
[ $? -eq 2 ] && ok "a check bundle with --open is refused" || bad "bundle check --open: $(head -2 "$W/out")"
# The canary: a root container on the host network, the role's credentials, one GetParameter.
cn=$RR/usr/local/libexec/slot-node/role-canary.sh
grep -q 'timeout 300 docker run --rm -i --network host --user 0:0 ' "$cn" && grep -q '^IMAGE=docker.io/curlimages/curl:[0-9.]*@sha256:[0-9a-f]\{64\}$' "$cn" && ok "the canary starts a root container on the host network from an image named by digest, with a time bound" || bad "canary container: $(grep 'docker run\|^IMAGE' "$cn" 2>&1)"
grep -q 'X-Amz-Target: AmazonSSM.GetParameter' "$cn" && grep -q -- ' -K - ' "$cn" && ok "the canary asks for a parameter and gives curl the credentials on standard input" || bad "canary request: $(grep 'GetParameter\|-K' "$cn" 2>&1)"
# What runs inside the container, with curl replaced: ci <status of the credentials list>
# <answer of Systems Manager>.
CI=$W/ci
mkdir -p "$CI/bin"
sed -n "/<<'IN'\$/,/^IN\$/p" "$cn" | sed '1d;$d' >"$CI/inner.sh"
cat >"$CI/bin/curl" <<EOF
#!/bin/sh
CI=$CI
EOF
cat >>"$CI/bin/curl" <<'EOF'
printf '%s\n' "$*" >>"$CI/argv"
for u; do :; done
case " $* " in *" -w "*) w=1 ;; *) w= ;; esac
case $u in
  */api/token)
    [ -f "$CI/no-token" ] && exit 22
    printf SESSIONTOKEN
    ;;
  */meta-data/iam/security-credentials/) if [ -n "$w" ]; then cat "$CI/code"; else echo slot-node-node-1; fi ;;
  */meta-data/placement/region) printf eu-test-1 ;;
  */meta-data/iam/security-credentials/*) cat "$CI/creds" ;;
  https://ssm.eu-test-1.amazonaws.com/)
    cat >"$CI/stdin"
    cat "$CI/ssm"
    ;;
esac
EOF
chmod 755 "$CI/bin/curl"
ci() {
  rm -f "$CI/argv" "$CI/stdin"
  printf '%s' "$1" >"$CI/code"
  printf '%s' "$2" >"$CI/ssm"
  printf '{\n  "Code" : "Success",\n  "AccessKeyId" : "ASIAKEYIDVALUE",\n  "SecretAccessKey" : "SECRET/KEY+VALUE",\n  "Token" : "ROLE/TOKEN+VALUE==",\n  "Expiration" : "2026-01-01T06:00:00Z"\n}\n' >"$CI/creds"
  [ -z "${3:-}" ] || : >"$CI/creds"
  PATH=$CI/bin:$PATH sh "$CI/inner.sh" >"$W/out" 2>"$W/err"
}
[ -s "$CI/inner.sh" ] || bad "the canary has no script for the container"
rm -f "$CI/no-token"
ci 200 '{"__type":"AccessDeniedException","Message":"not authorized to perform: ssm:GetParameter"}'
grep -qx 'canary: role slot-node-node-1' "$W/out" && grep -qx 'canary: answer AccessDeniedException' "$W/out" && grep -qx 'canary: end' "$W/out" && ok "in the container: the role is named and a refusal is read from an answer without a newline" || bad "inner refused: $(cat "$W/out" "$W/err")"
grep -q 'ASIAKEY\|SECRET\|ROLE/TOKEN' "$W/out" "$W/err" "$CI/argv" && bad "the role's credentials are in the output or in curl's arguments" || ok "in the container: the role's credentials are in no output line and in no argument of curl"
grep -qx 'user = "ASIAKEYIDVALUE:SECRET/KEY+VALUE"' "$CI/stdin" && grep -qx 'header = "x-amz-security-token: ROLE/TOKEN+VALUE=="' "$CI/stdin" && ok "in the container: curl reads the credentials from its standard input" || bad "inner stdin: $(cat "$CI/stdin" 2>&1)"
grep -q -- '--aws-sigv4 aws:amz:eu-test-1:ssm .*https://ssm.eu-test-1.amazonaws.com/$' "$CI/argv" && ok "in the container: the call is signed for Systems Manager of the instance's region" || bad "inner call: $(grep ssm "$CI/argv")"
ci 200 '{"Parameter":{"Name":"slot-node-role-canary","Value":"v"}}'
grep -qx 'canary: answer Parameter' "$W/out" && ok "in the container: a parameter that comes back is read as allowed" || bad "inner allowed: $(cat "$W/out")"
ci 200 '{"__type":"ParameterNotFound"}'
grep -qx 'canary: answer ParameterNotFound' "$W/out" && ok "in the container: ParameterNotFound is read as its own answer" || bad "inner not found: $(cat "$W/out")"
ci 404 ''
[ "$(cat "$W/out")" = 'canary: no-role' ] && ok "in the container: 404 for the credentials list is an instance without a role" || bad "inner 404: $(cat "$W/out")"
ci 500 '' empty
[ "$(cat "$W/out")" = 'canary: no-credentials 500' ] && ok "in the container: another status without credentials is not an instance without a role" || bad "inner 500: $(cat "$W/out")"
: >"$CI/no-token"
ci 200 ''
[ "$(cat "$W/out")" = 'canary: no-token' ] && ok "in the container: no token is not an instance without a role" || bad "inner no token: $(cat "$W/out")"
rm -f "$CI/no-token"
mkdir -p "$W/cn/bin"
cat >"$W/cn/bin/docker" <<EOF
#!/bin/sh
cat >/dev/null
echo "\$*" >>"$W/cn/calls"
cat "$W/cn/answer"
exit \$(cat "$W/cn/exit")
EOF
chmod 755 "$W/cn/bin/docker"
# cnr <what the container prints> [<exit status of docker run>]
cnr() {
  printf '%b\n' "$1" >"$W/cn/answer"
  echo "${2:-0}" >"$W/cn/exit"
  PATH=$W/cn/bin:$PATH sh "$cn" >"$W/out" 2>"$W/err"
}
cnr 'canary: role slot-node-node-1\ncanary: answer AccessDeniedException\ncanary: end'
st=$?
[ $st -eq 0 ] && grep -q '^ok: role slot-node-node-1: .*refused ssm:GetParameter (AccessDenied)\. One call: role.sh check reads the role$' "$W/out" && [ ! -s "$W/err" ] && ok "canary: a refused call is exit 0, names the role and says that it is one call" || bad "canary refused: exit $st $(cat "$W/out" "$W/err")"
for allowed in ParameterNotFound Parameter; do
  cnr "canary: role SharedManagedRole\ncanary: answer $allowed\ncanary: end"
  st=$?
  [ $st -eq 1 ] && grep -qx 'role canary: role SharedManagedRole may read parameters: Systems Manager allowed ssm:GetParameter to a container' "$W/err" && ok "canary: an allowed call ($allowed) is exit 1" || bad "canary $allowed: exit $st $(cat "$W/out" "$W/err")"
done
cnr 'canary: no-role'
st=$?
[ $st -eq 0 ] && grep -q '^ok: the instance has no role' "$W/out" && ok "canary: an instance without a role is exit 0" || bad "canary no role: exit $st $(cat "$W/out" "$W/err")"
cnr 'canary: no-token'
st=$?
[ $st -eq 1 ] && grep -q 'no token: nothing was tested$' "$W/err" && ok "canary: no token from the metadata service is exit 1, nothing tested" || bad "canary no token: exit $st $(cat "$W/out" "$W/err")"
cnr 'canary: no-credentials 200'
[ $? -eq 1 ] && grep -q 'nothing was tested$' "$W/err" && ok "canary: no credentials or no region is exit 1" || bad "canary no credentials: $(cat "$W/out" "$W/err")"
cnr 'canary: role r1\ncanary: answer ThrottlingException\ncanary: end'
st=$?
[ $st -eq 1 ] && grep -qx 'role canary: role r1: no answer the canary knows: canary: answer ThrottlingException' "$W/err" && ok "canary: another answer is exit 1 and is named" || bad "canary other answer: exit $st $(cat "$W/err")"
cnr 'canary: role r1\ncanary: end'
[ $? -eq 1 ] && ok "canary: no answer from Systems Manager is exit 1" || bad "canary no answer: $(cat "$W/out" "$W/err")"
cnr 'docker: Cannot connect to the Docker daemon' 125
st=$?
[ $st -eq 1 ] && grep -q '^role canary: the container did not run: docker: Cannot connect' "$W/err" && ok "canary: a container that does not run is exit 1" || bad "canary no container: exit $st $(cat "$W/err")"
# check --canary runs it; check alone starts no container. Under NODE_TEST=1 the canary runs
# only the stand-in for docker that the test names (NODE_TEST_DOCKER, the directory that holds
# it): a test or a probe that names none starts nothing, whatever `docker` its PATH has. crun
# puts the stand-in first on PATH as well, so that no change of provision.sh makes this file
# start a container.
crun() { NODE_TEST_DOCKER=$W/cn/bin PATH=$W/cn/bin:$PATH NODE_TEST=1 NODE_ROOT=$RR sh "$P" check "$W/node.conf" "$@" >"$W/out" 2>"$W/err"; }
printf 'canary: role slot-node-node-1\ncanary: answer AccessDeniedException\ncanary: end\n' >"$W/cn/answer"
echo 0 >"$W/cn/exit"
: >"$W/cn/calls"
crun
st=$?
[ $st -eq 0 ] && [ ! -s "$W/cn/calls" ] && ok "check without --canary starts no container" || bad "check started a container: exit $st $(cat "$W/cn/calls" "$W/out")"
PATH=$W/cn/bin:$PATH NODE_TEST=1 NODE_ROOT=$RR sh "$P" check "$W/node.conf" --canary >"$W/out" 2>"$W/err"
st=$?
[ $st -eq 1 ] && [ ! -s "$W/cn/calls" ] && grep -qx 'drift: role: canary: NODE_TEST=1 names no stand-in for docker (NODE_TEST_DOCKER): nothing was started' "$W/out" && ok "NODE_TEST=1 without NODE_TEST_DOCKER: check --canary runs no docker, also one that is first on PATH, and says so" || bad "canary without a named stand-in: exit $st calls '$(cat "$W/cn/calls")' $(grep canary "$W/out")"
crun --canary
st=$?
[ $st -eq 0 ] && grep -q '^canary: ok: role slot-node-node-1: ' "$W/out" && grep -q '^0 drift$' "$W/out" && [ "$(wc -l <"$W/cn/calls")" -eq 1 ] && ok "check --canary with a refused call: one container, the canary line of the role, 0 drift" || bad "check --canary: exit $st $(cat "$W/out" "$W/err")"
printf 'canary: role slot-node-node-1\ncanary: answer ParameterNotFound\ncanary: end\n' >"$W/cn/answer"
crun --canary
st=$?
[ $st -eq 1 ] && grep -qx 'drift: role: canary: role slot-node-node-1 may read parameters: Systems Manager allowed ssm:GetParameter to a container' "$W/out" && ok "check --canary with an allowed call: drift, exit 1" || bad "check --canary allowed: exit $st $(cat "$W/out" "$W/err")"
rm -f "$RR/.state/active/docker.service"
: >"$W/cn/calls"
crun --canary
grep -qx 'drift: role: canary: docker.service is not active: nothing was tested' "$W/out" && [ ! -s "$W/cn/calls" ] && ok "check --canary without a running docker says that nothing was tested" || bad "canary without docker: $(cat "$W/out")"
: >"$RR/.state/active/docker.service"
# The musl canary: check --canary writes one C file that includes stdio.h, compiles it with
# musl-gcc -static as the build user in a directory of the store's tmp, runs the result and
# removes the directory. The wrapper apply writes under NODE_TEST=1 is a stand-in that makes a
# script of the line the source prints; the two failures are stand-ins of this test, which record
# how they were called.
printf 'canary: role slot-node-node-1\ncanary: answer AccessDeniedException\ncanary: end\n' >"$W/cn/answer"
MUW=$RR/usr/local/musl-1.2.6/bin/musl-gcc
crun --canary
st=$?
[ $st -eq 0 ] && grep -qx 'canary: ok: musl: a C file that includes stdio.h, compiled with musl-gcc -static (musl 1.2.6), runs and prints its line' "$W/out" && grep -q '^0 drift$' "$W/out" && ok "check --canary: the musl canary compiles, runs and is one ok line, 0 drift" || bad "musl canary: exit $st $(cat "$W/out" "$W/err" | tail -5)"
[ -d "$RR/srv/slot/tmp" ] && [ -z "$(ls -A "$RR/srv/slot/tmp" | grep 'musl')" ] && ok "the musl canary leaves no directory in the store's tmp" || bad "musl canary left: $(ls -A "$RR/srv/slot/tmp" 2>&1)"
cp "$MUW" "$W/musl-gcc.keep"
cat >"$MUW" <<EOF
#!/bin/sh
[ "\$1" != --version ] || { echo "gcc (stand-in) 11.5.0"; exit 0; }
echo "\$*" >"$W/musl-args"
pwd >"$W/musl-pwd"
cat canary.c >"$W/musl-src" 2>/dev/null
echo "canary.c: In function 'main':" >&2
echo "canary.c:1:10: fatal error: stdio.h: No such file or directory" >&2
echo "compilation terminated." >&2
exit 1
EOF
rm -f "$W/musl-args"
crun
[ ! -e "$W/musl-args" ] && ! grep -q 'canary' "$W/out" && ok "check without --canary compiles nothing" || bad "check alone ran the musl canary: $(cat "$W/musl-args" 2>&1) $(grep canary "$W/out")"
crun --canary
st=$?
[ $st -eq 1 ] && grep -qx 'drift: musl: canary: musl-gcc -static canary.c failed: canary.c:1:10: fatal error: stdio.h: No such file or directory' "$W/out" && ! grep -q '^canary: ok: musl' "$W/out" && ok "check --canary: a compile that fails is drift with the compiler's first error line" || bad "musl canary compile error: exit $st $(grep musl "$W/out" "$W/err")"
[ "$(cat "$W/musl-args" 2>&1)" = '-static -o canary canary.c' ] && grep -qx '#include <stdio.h>' "$W/musl-src" && ok "the musl canary compiles a file that includes stdio.h with -static" || bad "musl canary call: $(cat "$W/musl-args" "$W/musl-src" 2>&1)"
case $(cat "$W/musl-pwd" 2>&1) in
  "$RR"/srv/slot/tmp/slot-musl-canary.*) [ ! -e "$(cat "$W/musl-pwd")" ] && ok "the musl canary compiles in a directory of the store's tmp and removes it after a failure too" || bad "musl canary directory left: $(cat "$W/musl-pwd")" ;;
  *) bad "musl canary directory: $(cat "$W/musl-pwd" 2>&1)" ;;
esac
cat >"$MUW" <<EOF
#!/bin/sh
[ "\$1" != --version ] || { echo "gcc (stand-in) 11.5.0"; exit 0; }
echo "warning: a line that is no failure" >&2
printf '#!/bin/sh\necho "another line"\n' >canary && chmod 755 canary
EOF
crun --canary
st=$?
[ $st -eq 1 ] && grep -qx "drift: musl: canary: the program that musl-gcc -static made prints 'another line', want 'slot musl canary'" "$W/out" && ! grep -q '^canary: ok: musl' "$W/out" && ok "check --canary: a program that prints another line is drift" || bad "musl canary output: exit $st $(grep musl "$W/out" "$W/err")"
cp "$W/musl-gcc.keep" "$MUW"
sh "$P" bundle "$W/node.conf" check --canary >"$W/bundle-canary.sh"
grep -qx 'sh "$d/provision.sh" check "$d/node.conf" --canary' "$W/bundle-canary.sh" && ok "a check bundle carries --canary" || bad "bundle --canary: $(tail -4 "$W/bundle-canary.sh" | head -1)"

# 6r. glab and gh work with a credential, and no credential goes to a node: each is a script in
# nodebin, the first directory of a job's PATH, that says so in one line and exits 255. A task
# that calls one ends with that line, not with what its next command makes of an empty answer.
for b in glab gh; do
  nt=$RR/srv/slot/nodebin/$b
  line="slot: node-1 has no $b: it works with a credential, and no credential goes to the node. Run the step that calls it on the workstation: slot --here <command>, or name the command in .slot.toml local = [...] of its repository."
  printf '#!/bin/sh\necho "%s" >&2\nexit 255\n' "$line" | cmp -s - "$nt" && [ -x "$nt" ] && ok "$b: nodebin has the script, executable" || bad "$b: $(ls -l "$nt" 2>&1) $(cat "$nt" 2>&1)"
  msg=$("$nt" api projects/1/merge_requests 2>&1 >"$W/out")
  st=$?
  [ $st -eq 255 ] && [ ! -s "$W/out" ] && [ "$msg" = "$line" ] && ok "$b: run, it prints the one line on stderr, nothing on stdout, and exits 255" || bad "$b: exit $st '$msg' $(cat "$W/out")"
done
# nodebin is the first directory of a job's PATH: a name in it replaces that tool for every job.
[ "$(ls "$RR/srv/slot/nodebin" | tr '\n' ' ')" = 'gh glab no-credentials ssh ' ] && ok "nodebin holds gh, glab, no-credentials and ssh and nothing else: no other tool of a job is replaced" || bad "nodebin: $(ls "$RR/srv/slot/nodebin" | tr '\n' ' ')"

# 6s. Core dumps. A bare node pipes every core to systemd-coredump, and the kernel runs a pipe
# handler whatever the core limit of the process is: an aborted process is reaped only after the
# handler ran. The node's pattern is the file name `core`, and slot-node.service and
# sccache-node.service run with the soft core limit 0 (section 2): an aborted process of a job
# writes nothing and is reaped at once. Under the pattern `core` a process whose limit is not 0
# writes a core file, so the pattern is loaded only once both units run with the limit 0.
# `check --canary` times an abort and reads the limit of slot-node. The role canary of this root
# answers with a refusal again.
printf 'canary: role slot-node-node-1\ncanary: answer AccessDeniedException\ncanary: end\n' >"$W/cn/answer"
# fn NAME...: the text of functions of provision.sh, for a test that runs one of them alone with
# stand-ins for what it calls.
fn() {
  for fn_n in "$@"; do sed -n "/^$fn_n() {\$/,/^}\$/p" "$P"; done
}
# A node provisioned before the core limit: unit files without one, both processes with the
# limit unlimited, a pipe as the pattern (any handler counts, not only systemd's), 3 jobs running.
pipe='|/usr/share/apport/apport -p%p -s%s -c%c'
sysctl_f=/etc/sysctl.d/60-slot-core.conf
echo "$pipe" >"$RR/.state/core-pattern"
mkdir -p "$RR/.state/core-limit"
for u in slot-node.service sccache-node.service; do
  sed -i '/^LimitCORE=/d' "$RR/etc/systemd/system/$u"
  echo unlimited >"$RR/.state/core-limit/$u"
done
echo 3 >"$RR/.state/jobs"
rm -f "$RR$sysctl_f"
: >"$RR/.calls"
run "$RR" apply
st=$?
[ "$(cat "$RR$sysctl_f" 2>&1)" = 'kernel.core_pattern=core' ] && ok "a pipe of another handler than systemd's, no sysctl file, jobs running: apply writes the file for the next boot" || bad "sysctl file with jobs: $(cat "$RR$sysctl_f" 2>&1)"
why='the process has the core limit unlimited, not 0'
[ $st -eq 0 ] && grep -qx 'LimitCORE=0:infinity' "$RR/etc/systemd/system/slot-node.service" && grep -qx 'LimitCORE=0:infinity' "$RR/etc/systemd/system/sccache-node.service" && ! grep -q 'systemctl restart' "$RR/.calls" && grep -qx "pending: running: sccache-node restarts when no job runs (3 now): $why" "$W/err" && grep -qx "pending: running: slot-node restarts when no job runs (3 now): $why" "$W/err" && ok "jobs running: both unit files get the core limit, no unit restarts, a pending line for each" || bad "core limit with jobs: exit $st $(grep LimitCORE "$RR/etc/systemd/system/slot-node.service" "$RR/etc/systemd/system/sccache-node.service") $(cat "$RR/.calls" "$W/err")"
! grep -q sysctl "$RR/.calls" && [ "$(cat "$RR/.state/core-pattern")" = "$pipe" ] && grep -qx "pending: core: kernel.core_pattern stays '$pipe' while sccache-node.service runs with the core limit unlimited (under the pattern core its processes write core files): the apply that restarts it loads $sysctl_f" "$W/err" && ! grep -q '^changed: core: kernel' "$W/out" && ok "jobs running: the pattern is not loaded while a unit runs with another core limit than 0, a pending line says so" || bad "pattern with jobs: $(cat "$RR/.state/core-pattern") $(grep sysctl "$RR/.calls") $(grep core "$W/out" "$W/err")"
crun --canary
st=$?
[ $st -eq 1 ] && grep -qx "drift: running: slot-node.service: $why" "$W/out" && grep -qx "drift: running: sccache-node.service: $why" "$W/out" && grep -qx "drift: core: kernel.core_pattern is '$pipe', a pipe: an aborted process waits for the handler" "$W/out" && ok "check names each running unit whose core limit is not 0, and a pipe of another handler than systemd's" || bad "check with stale core limits: exit $st $(cat "$W/out")"
grep -qx "drift: core: canary: slot-node.service runs with the core limit 'unlimited', not 0: an aborted process of a job writes a core file" "$W/out" && ! grep -q '^canary: ok: core' "$W/out" && ok "check --canary reads the core limit of the running slot-node: not 0 is drift, no ok line" || bad "canary with a stale core limit: $(grep 'core' "$W/out")"
# No job: the units restart first, then the pattern is loaded. Here the load does not take.
echo 0 >"$RR/.state/jobs"
: >"$RR/.state-core-stays"
: >"$RR/.calls"
run "$RR" apply
st=$?
[ "$(grep 'systemctl restart\|sysctl' "$RR/.calls" | tr '\n' ',')" = "systemctl restart sccache-node.service,systemctl restart slot-node.service,sysctl -q -p $sysctl_f," ] && grep -qx "changed: running: sccache-node.service restarted: $why" "$W/out" && grep -qx "changed: running: slot-node.service restarted: $why" "$W/out" && ok "no job: one apply restarts both units and then loads the pattern" || bad "restart, then load: $(cat "$RR/.calls") $(grep running "$W/out" "$W/err")"
[ $st -eq 1 ] && grep -qx "failed: core: kernel.core_pattern is still '$pipe' after sysctl -p $sysctl_f" "$W/err" && ! grep -q '^changed: core' "$W/out" && ok "a pattern that is still a pipe after the load: failed line, exit 1, not reported as changed" || bad "core stays a pipe: exit $st $(grep core "$W/out" "$W/err")"
rm -f "$RR/.state-core-stays"
: >"$RR/.calls"
run "$RR" apply
st=$?
[ $st -eq 0 ] && [ "$(cat "$RR/.calls")" = "sysctl -q -p $sysctl_f" ] && grep -qx "changed: core: kernel.core_pattern is 'core' (sysctl -p $sysctl_f)" "$W/out" && ok "a pipe with the file in place and both units at the core limit 0: apply loads the file, no restart" || bad "core pipe again: exit $st $(grep core "$W/out" "$W/err") $(cat "$RR/.calls")"
echo 50 >"$RR/.state/abort-ms"
crun --canary
st=$?
[ $st -eq 0 ] && grep -qx 'canary: ok: core: an aborted process is reaped in 50 ms (the quickest of 5), and slot-node.service runs with the core limit 0' "$W/out" && grep -q '^canary: ok: role slot-node-node-1: ' "$W/out" && grep -q '^0 drift$' "$W/out" && ok "after it: 0 drift, and check --canary has an ok line for 50 ms and the core limit 0 beside the role's" || bad "core canary 50: exit $st $(cat "$W/out" "$W/err")"
echo 51 >"$RR/.state/abort-ms"
crun --canary
st=$?
[ $st -eq 1 ] && grep -qx 'drift: core: canary: an aborted process is reaped after 51 ms (the quickest of 5), over 50' "$W/out" && grep -qx '1 drift' "$W/out" && ok "check --canary: an abort reaped after 51 ms is drift" || bad "core canary 51: exit $st $(cat "$W/out" "$W/err")"
# A pattern that is neither a pipe nor `core` is the owner's choice: it stays, and with no sysctl
# file there none is written (the file would make the pattern `core` at the next boot). One line
# says so. Without --canary nothing is timed.
echo '/var/crash/core.%e.%p' >"$RR/.state/core-pattern"
rm -f "$RR$sysctl_f"
kept="kept: core: kernel.core_pattern is '/var/crash/core.%e.%p', neither a pipe nor core: it stays, and $sysctl_f is not written"
run "$RR" check
st=$?
[ $st -eq 0 ] && grep -q '^0 drift$' "$W/out" && grep -qx "$kept" "$W/out" && ! grep -q canary "$W/out" && ok "another pattern that is no pipe, and no sysctl file: no drift, one line says that both stay, and check without --canary times no abort" || bad "owner's pattern: exit $st $(cat "$W/out")"
: >"$RR/.calls"
run "$RR" apply
[ ! -s "$RR/.calls" ] && [ ! -e "$RR$sysctl_f" ] && [ "$(cat "$RR/.state/core-pattern")" = '/var/crash/core.%e.%p' ] && grep -qx "$kept" "$W/out" && ok "apply leaves another pattern that is no pipe alone and writes no sysctl file" || bad "owner's pattern after apply: $(cat "$RR/.calls" "$RR/.state/core-pattern") $(ls "$RR$sysctl_f" 2>&1) $(grep core "$W/out")"
rm -f "$RR/.state/abort-ms"
echo core >"$RR/.state/core-pattern"
echo 'kernel.core_pattern=core' >"$RR$sysctl_f"
# The canary's answers one by one: p_core_canary alone, with $1 as the time, $2 as the core limit
# of slot-node and, with a third word, a slot-node that is not active.
cc_fn=$(fn p_core_canary)
core_canary() {
  (
    eval "$cc_fn"
    mode=check canary=1 cc_ms=$1 cc_l=$2 cc_off=${3:-}
    drift() { echo "drift: $1: $2"; }
    q_abort_ms() { echo "$cc_ms"; }
    q_core_limit() { echo "$cc_l"; }
    q_active() { [ -z "$cc_off" ]; }
    p_core_canary 2>&1
  )
}
[ "$(core_canary 50 0)" = 'canary: ok: core: an aborted process is reaped in 50 ms (the quickest of 5), and slot-node.service runs with the core limit 0' ] && [ "$(core_canary 51 0)" = 'drift: core: canary: an aborted process is reaped after 51 ms (the quickest of 5), over 50' ] && ok "the canary's bound: 50 ms is ok, 51 ms is drift" || bad "canary bound: $(core_canary 50 0) / $(core_canary 51 0)"
for none in '' 'date: invalid' ' 7' -0 0x10; do
  [ "$(core_canary "$none" 0)" = 'drift: core: canary: no time was measured for an aborted process: nothing was tested' ] && ok "canary: '$none' for a time is drift, not ok" || bad "core canary '$none': $(core_canary "$none" 0)"
done
big=99999999999999999999
[ "$(core_canary $big 0)" = "drift: core: canary: an aborted process is reaped after $big ms (the quickest of 5), over 50" ] && ok "canary: a time over the shell's integer range is drift, and the shell prints no error" || bad "core canary $big: $(core_canary $big 0)"
[ "$(core_canary 2 unlimited)" = "drift: core: canary: slot-node.service runs with the core limit 'unlimited', not 0: an aborted process of a job writes a core file" ] && [ "$(core_canary 2 '')" = "drift: core: canary: slot-node.service runs with the core limit 'unknown', not 0: an aborted process of a job writes a core file" ] && [ "$(core_canary 2 0 off)" = 'drift: core: canary: slot-node.service is not active: the core limit of its jobs was not read' ] && ok "canary: a core limit of slot-node that is not 0 or cannot be read, and a slot-node that is not active, are drift" || bad "canary limit: $(core_canary 2 unlimited) / $(core_canary 2 '') / $(core_canary 2 0 off)"
# The real branches (NODE_TEST unset), each alone with a stand-in for the system command it runs.
# q_abort_ms aborts under the soft core limit 0, whatever limit provision.sh was started with:
# here `sh` prints its soft limit instead of aborting. Where this run's hard limit is 0 no other
# limit can be set first, and the case passes untested.
al=$(
  ulimit -S -c unlimited 2>/dev/null || ulimit -S -c 1024 2>/dev/null
  if [ "$(ulimit -S -c)" = 0 ]; then
    echo hard0
    exit 0
  fi
  eval "$(fn q_abort_ms)"
  T=
  : >"$W/abort-limits"
  sh() { ulimit -S -c >>"$W/abort-limits"; }
  al_ms=$(q_abort_ms)
  echo "$al_ms $(sort -u "$W/abort-limits" | tr '\n' ' ')$(grep -c '' "$W/abort-limits") $(ulimit -S -c)"
)
case $al in
  hard0) ok "q_abort_ms: the soft core limit of its five runs (not tested: the hard core limit of this run is 0)" ;;
  [0-9]*' 0 5 '[!0]*) ok "q_abort_ms: five runs under the soft core limit 0, the quickest time in ms, and the caller's limit as it was" ;;
  *) bad "q_abort_ms: '$al' (want '<ms> 0 5 <the limit before>')" ;;
esac
# q_core_limit: the soft limit in /proc/<MainPID>/limits; `systemctl` answers this shell's pid.
cat >"$W/ql.sh" <<'EOF'
T=
eval "$(sed -n '/^q_core_limit() {$/,/^}$/p' "$1")"
systemctl() { echo $$; }
ulimit -S -c 0
a=$(q_core_limit x.service)
if ulimit -S -c unlimited 2>/dev/null; then b=$(q_core_limit x.service); else b=unlimited; fi
systemctl() { echo 0; }
c=$(q_core_limit x.service)
echo "$a $b '$c'"
EOF
ql=$(sh "$W/ql.sh" "$P" 2>&1)
[ "$ql" = "0 unlimited ''" ] && ok "q_core_limit: the soft core limit of the unit's main process (0, unlimited), nothing without a process" || bad "q_core_limit: $ql"
sl=$(
  eval "$(fn a_sysctl_load)"
  T=
  sysctl() { echo "sysctl $*"; }
  a_sysctl_load "$sysctl_f"
)
[ "$sl" = "sysctl -q -p $sysctl_f" ] && ok "the load without NODE_TEST is sysctl -q -p of the file" || bad "a_sysctl_load: $sl"

# 6t. The pool of a node holds a queued job while the host is busy: `slot cpu-limit` with 80 % of
# the CPUs and 20 % CPU pressure (sections 2 and 3), or the marks the config names (CPU_LIMIT,
# CPU_PRESSURE). check asks the daemon which marks it holds (GET /api/cpu-limit as the build
# user, 5 s at most): root opens no file of the build user's state directory. Under NODE_TEST=1
# the answer is made from the files cpu-limit and cpu-pressure, and there is one only while
# slot-node.service is active. This root has 2 CPUs.
[ "$(cat "$RR/.state/cpu-limit" 2>&1)" = 1.6 ] && [ "$(cat "$RR/.state/cpu-pressure" 2>&1)" = 20 ] && ok "2 CPUs: the marks are 1.6 busy CPUs and 20 % CPU pressure" || bad "2 CPUs: $(cat "$RR/.state/cpu-limit" "$RR/.state/cpu-pressure" 2>&1)"
# The daemon answers 20.0 for 20: marks are compared as numbers. Without --canary the load is not
# read.
echo 1.60 >"$RR/.state/cpu-limit"
echo 20.0 >"$RR/.state/cpu-pressure"
echo '0.10 99.00 0.30 1/100 1' >"$RR/.state/loadavg"
run "$RR" check
st=$?
[ $st -eq 0 ] && grep -q '^0 drift$' "$W/out" && ! grep -q canary "$W/out" && ok "the same marks in another notation (1.60, 20.0) are no drift, and check without --canary reads no load" || bad "notation: exit $st $(cat "$W/out")"
printf 'CPU_LIMIT=20\nCPU_PRESSURE=62.5\n' >>"$W/node.conf"
run "$RR" check
st=$?
[ $st -eq 1 ] && grep -qx "drift: cpu-limit: the pool holds a queued job from '1.60' busy CPUs and '20.0' % CPU pressure, want '20' and '62.5'" "$W/out" && ok "CPU_LIMIT and CPU_PRESSURE name other marks: check names the difference" || bad "explicit marks: exit $st $(cat "$W/out")"
: >"$RR/.calls"
run "$RR" apply
st=$?
[ $st -eq 0 ] && grep -qx 'slot cpu-limit 20 --pressure 62.5' "$RR/.calls" && grep -qx 'changed: cpu-limit: a queued job is held from 20 busy CPUs and 62.5 % CPU pressure (slot cpu-limit 20 --pressure 62.5)' "$W/out" && [ "$(cat "$RR/.state/cpu-limit")" = 20 ] && ok "apply sets the marks of the config" || bad "explicit marks apply: exit $st $(cat "$RR/.calls" "$W/out" "$W/err")"
# Only the pressure mark differs: that is drift too, before the call and after it.
conf
printf 'CPU_LIMIT=20\nCPU_PRESSURE=off\n' >>"$W/node.conf"
: >"$RR/.calls"
run "$RR" apply
st=$?
[ $st -eq 0 ] && grep -qx 'slot cpu-limit 20 --pressure off' "$RR/.calls" && [ "$(cat "$RR/.state/cpu-limit")" = 20 ] && [ ! -e "$RR/.state/cpu-pressure" ] && ok "CPU_PRESSURE=off with the CPU mark as it is: apply takes the pressure mark away" || bad "pressure off: exit $st $(cat "$RR/.calls" "$W/out" "$W/err")"
conf
printf 'CPU_LIMIT=20\nCPU_PRESSURE=62.5\n' >>"$W/node.conf"
: >"$RR/.state-cpu-limit-stays"
run "$RR" apply
st=$?
[ $st -eq 1 ] && grep -qx "failed: cpu-limit: after slot cpu-limit 20 --pressure 62.5 the pool holds a queued job from '20' busy CPUs and 'off' % CPU pressure" "$W/err" && ! grep -q '^changed: cpu-limit' "$W/out" && ok "a pressure mark the pool does not have after the call: failed line, exit 1, not reported as changed" || bad "marks stay: exit $st $(grep cpu-limit "$W/out" "$W/err")"
rm -f "$RR/.state-cpu-limit-stays"
conf
echo 'CPU_LIMIT=off' >>"$W/node.conf"
run "$RR" check
grep -qx "drift: cpu-limit: the pool holds a queued job from '20' busy CPUs and 'off' % CPU pressure, want 'off' and 'off'" "$W/out" && ok "CPU_LIMIT=off: a mark the pool has is drift, and no pressure mark is wanted" || bad "off: $(cat "$W/out")"
: >"$RR/.calls"
run "$RR" apply
st=$?
[ $st -eq 0 ] && grep -qx 'slot cpu-limit off' "$RR/.calls" && grep -qx 'changed: cpu-limit: no CPU limit (slot cpu-limit off)' "$W/out" && [ ! -e "$RR/.state/cpu-limit" ] && ok "CPU_LIMIT=off: apply turns the limit off" || bad "off apply: exit $st $(cat "$RR/.calls" "$W/out" "$W/err")"
crun --canary
st=$?
[ $st -eq 0 ] && grep -q '^0 drift$' "$W/out" && ! grep -q 'cpu-limit' "$W/out" && ok "CPU_LIMIT=off: 0 drift, and check --canary has no line about the load" || bad "off canary: exit $st $(cat "$W/out" "$W/err")"
# A daemon that does not run gives no answer: check says so, and apply does not call it.
conf
rm -f "$RR/.state/active/slot-node.service"
: >"$RR/.state-start-fails-slot-node.service"
run "$RR" check
grep -qx "drift: cpu-limit: the daemon of the pool gives no answer about its CPU limit (GET http://127.0.0.1:7411/api/cpu-limit as $me, 5 s at most), want '1.6' and '20'" "$W/out" && ok "with slot-node.service not active no daemon answers: check says so and names the limit it wants" || bad "not active, check: $(cat "$W/out")"
: >"$RR/.calls"
run "$RR" apply
st=$?
[ $st -eq 1 ] && grep -qx 'failed: unit:slot-node.service: systemctl start slot-node.service' "$W/err" && grep -qx 'failed: cpu-limit: slot-node.service is not active: the limit was not set' "$W/err" && ! grep -q 'cpu-limit' "$RR/.calls" && ok "a slot-node.service that does not start: failed lines for the unit and the limit, no slot cpu-limit call" || bad "not active, apply: exit $st $(cat "$W/err" "$RR/.calls")"
rm -f "$RR/.state-start-fails-slot-node.service"
run "$RR" apply
st=$?
[ $st -eq 0 ] && grep -qx 'slot cpu-limit 1.6 --pressure 20' "$RR/.calls" && [ "$(cat "$RR/.state/cpu-limit" 2>&1)" = 1.6 ] && ok "once slot-node.service starts, apply sets the limit" || bad "limit after the start: exit $st $(cat "$RR/.calls" "$W/out" "$W/err")"
# check --canary reads the 5-minute load average, the second number of /proc/loadavg: over 2.5
# per CPU the limit does not hold the node.
echo '9.99 4.20 0.01 1/100 1' >"$RR/.state/loadavg"
crun --canary
st=$?
[ $st -eq 0 ] && grep -qx 'canary: ok: cpu-limit: the 5-minute load average is 4.20 on 2 CPUs (bound 5)' "$W/out" && grep -q '^0 drift$' "$W/out" && ok "check --canary: a 5-minute load under 2.5 per CPU is an ok line, whatever the 1-minute load is" || bad "load 4.20: exit $st $(cat "$W/out" "$W/err")"
echo '0.01 5.01 0.02 1/100 1' >"$RR/.state/loadavg"
crun --canary
st=$?
[ $st -eq 1 ] && grep -qx 'drift: cpu-limit: canary: the 5-minute load average is 5.01 on 2 CPUs, over 5 (2.5 per CPU): the limit does not hold the node' "$W/out" && grep -qx '1 drift' "$W/out" && ok "check --canary: a load over the bound is drift" || bad "load 5.01: exit $st $(cat "$W/out" "$W/err")"
rm -f "$RR/.state/loadavg"
# The load canary's answers one by one: p_cpu_limit_canary alone on a loadavg line whose second
# number is $1, with $2 CPUs and the wanted mark $3.
lc_fn=$(fn is_decimal q_load5 p_cpu_limit_canary)
load_canary() {
  (
    eval "$lc_fn"
    T=1 S=$W/lc mode=check canary=1 lc_n=$2 lc_w=${3:-1.6}
    mkdir -p "$S"
    printf '0.11 %s 0.33 2/345 6789\n' "$1" >"$S/loadavg"
    drift() { echo "drift: $1: $2"; }
    q_nproc() { echo "$lc_n"; }
    cpu_limit_want() { echo "$lc_w"; }
    p_cpu_limit_canary 2>&1
  )
}
[ "$(load_canary 5.00 2)" = 'canary: ok: cpu-limit: the 5-minute load average is 5.00 on 2 CPUs (bound 5)' ] && [ "$(load_canary 5.01 2)" = 'drift: cpu-limit: canary: the 5-minute load average is 5.01 on 2 CPUs, over 5 (2.5 per CPU): the limit does not hold the node' ] && ok "the load canary's bound: a load at 2.5 per CPU is ok, over it is drift" || bad "load bound: $(load_canary 5.00 2) / $(load_canary 5.01 2)"
[ "$(load_canary 80.00 32)" = 'canary: ok: cpu-limit: the 5-minute load average is 80.00 on 32 CPUs (bound 80)' ] && [ "$(load_canary 80.01 32)" = 'drift: cpu-limit: canary: the 5-minute load average is 80.01 on 32 CPUs, over 80 (2.5 per CPU): the limit does not hold the node' ] && ok "the load canary: 32 CPUs have the bound 80" || bad "load on 32 CPUs: $(load_canary 80.00 32) / $(load_canary 80.01 32)"
miss=
for none in '' x . .. 1e3 +3 -1 0x10 4.2.0 .5 5.; do
  [ "$(load_canary "$none" 2)" = 'drift: cpu-limit: canary: the 5-minute load average could not be read: nothing was tested' ] || miss="$miss '$none': $(load_canary "$none" 2);"
done
[ -z "$miss" ] && ok "the load canary: an answer that is no plain number (nothing, x, ., .., 1e3, +3, -1, 0x10, 4.2.0, .5, 5.) is drift, not ok" || bad "load answers:$miss"
[ -z "$(load_canary 99.00 2 off)" ] && ok "the load canary: CPU_LIMIT=off has no line" || bad "load canary with the limit off: $(load_canary 99.00 2 off)"
lr=no
for try in 1 2 3; do
  [ "$(
    eval "$(fn q_load5)"
    T=
    q_load5
  )" = "$(cut -d' ' -f2 /proc/loadavg)" ] && lr=yes && break
done
[ $lr = yes ] && ok "without NODE_TEST the load is the second number of /proc/loadavg" || bad "q_load5 without NODE_TEST is not the second number of /proc/loadavg"
# The daemon's answer, read alone (cpu_mark): the two marks of the object GET /api/cpu-limit
# answers with. null is a mark that is off.
cm_fn=$(fn is_decimal cpu_mark)
mark() {
  (
    eval "$cm_fn"
    cpu_mark "$1" "$2"
  )
}
ans='{"cpu_busy_max":25.6,"cpu_busy_floor":4,"cpus":32,"busy":3.1,"held":0,"pool":2.0,"compile_cache":0.5,"outside":0.6,"cpu_pressure_max":40.0,"pressure":1.5}'
[ "$(mark cpu_busy_max "$ans"):$(mark cpu_pressure_max "$ans")" = 25.6:40.0 ] && ok "the daemon's answer: the CPU mark and the pressure mark, not the floor, the CPUs or the pressure now" || bad "marks of the answer: $(mark cpu_busy_max "$ans"):$(mark cpu_pressure_max "$ans")"
ans_off='{"cpu_busy_max":null,"cpu_busy_floor":null,"cpus":32,"busy":3.1,"held":0,"pool":2.0,"compile_cache":0.5,"outside":0.6,"cpu_pressure_max":null,"pressure":1.5}'
ans_wide=$(printf '{\n  "cpu_pressure_max" : 40,\n  "cpus" : 32,\n  "cpu_busy_max" : 25.60\n}\n')
[ "$(mark cpu_busy_max "$ans_off"):$(mark cpu_pressure_max "$ans_off")" = off:off ] && [ "$(mark cpu_busy_max "$ans_wide"):$(mark cpu_pressure_max "$ans_wide")" = 25.60:40 ] && ok "the daemon's answer: null is off, and the keys are read in any order and layout" || bad "null and layout: $(mark cpu_busy_max "$ans_off"):$(mark cpu_pressure_max "$ans_off") $(mark cpu_busy_max "$ans_wide"):$(mark cpu_pressure_max "$ans_wide")"
# p_cpu_limit alone in check mode, wanting 25.6 and 40, on the answer $1.
cl_fn=$(fn is_decimal cpu_mark same_mark cpu_limit_read cpu_limit_holds p_cpu_limit)
cpu_check() {
  (
    eval "$cl_fn"
    mode=check U=builder CPU_LIMIT_URL=http://127.0.0.1:7411/api/cpu-limit ca=$1
    drift() { echo "drift: $1: $2"; }
    fixing() { false; }
    cpu_limit_want() { echo 25.6; }
    cpu_pressure_want() { echo 40; }
    q_cpu_limit_answer() { printf '%s\n' "$ca"; }
    p_cpu_limit 2>&1
  )
}
[ -z "$(cpu_check "$ans")" ] && [ "$(cpu_check '{"cpu_busy_max":25.6,"cpu_busy_floor":4,"cpus":32}')" = "drift: cpu-limit: the pool holds a queued job from '25.6' busy CPUs and 'off' % CPU pressure, want '25.6' and '40'" ] && ok "the wanted marks are no drift; an answer of a daemon that knows no pressure mark is a pressure mark that is off" || bad "answer of the daemon: '$(cpu_check "$ans")' / $(cpu_check '{"cpu_busy_max":25.6,"cpu_busy_floor":4,"cpus":32}')"
na="drift: cpu-limit: the daemon of the pool gives no answer about its CPU limit (GET http://127.0.0.1:7411/api/cpu-limit as builder, 5 s at most), want '25.6' and '40'"
miss=
for a in '' '{}' 'not found' '{"cpu_busy_max":"25.6","cpu_pressure_max":40.0}' '{"cpu_busy_max":25.6,"cpu_pressure_max":"40"}' '{"cpu_busy_max":25.6e,"cpu_pressure_max":40}' '{"cpu_busy_max":-25.6,"cpu_pressure_max":40}' '{"cpu_busy_max":' '{"cpu_busy_max":25.6,"cpu_pressure_max":40' '{"cpu_busy_max":25.6,"cpu_pre'; do
  [ "$(cpu_check "$a")" = "$na" ] || miss="$miss '$a': $(cpu_check "$a");"
done
[ -z "$miss" ] && ok "an answer without a CPU mark, or with a mark that is no number (nothing, {}, a text, a string, 25.6e, -25.6, an object cut off at three places), is drift: no answer" || bad "answers that are none:$miss"
# p_cpu_limit alone in apply mode (NODE_TEST unset), wanting 25.6 and 40: the daemon gives no
# answer to the first $1 requests, then the wanted marks.
cpu_apply() {
  (
    eval "$cl_fn"
    T= mode=apply U=builder CPU_LIMIT_URL=http://127.0.0.1:7411/api/cpu-limit ca_n=$1
    : >"$W/ca-asked"
    drift() { :; }
    fixing() { true; }
    failed() { echo "failed: $1: $2"; }
    changed() { echo "changed: $1: $2"; }
    q_active() { true; }
    cpu_limit_want() { echo 25.6; }
    cpu_pressure_want() { echo 40; }
    a_cpu_limit() { echo "the daemon's last line"; }
    sleep() { echo "sleep $*"; }
    q_cpu_limit_answer() {
      echo asked >>"$W/ca-asked"
      [ "$(grep -c '' "$W/ca-asked")" -le "$ca_n" ] || printf '%s\n' "$ans"
    }
    p_cpu_limit 2>&1
  )
}
[ "$(cpu_apply 3 | tr '\n' ',')" = 'sleep 1,sleep 1,changed: cpu-limit: a queued job is held from 25.6 busy CPUs and 40 % CPU pressure (slot cpu-limit 25.6 --pressure 40),' ] && ok "apply: a daemon that answers only at the third request after the call is asked again, 1 s apart, and the limit is reported as set" || bad "cpu_apply 3: $(cpu_apply 3)"
[ "$(cpu_apply 99 | tr '\n' ',')" = "sleep 1,sleep 1,sleep 1,sleep 1,failed: cpu-limit: after slot cpu-limit 25.6 --pressure 40 the daemon of the pool gives no answer about its CPU limit: the daemon's last line," ] && [ "$(grep -c '' "$W/ca-asked")" = 6 ] && ok "apply: a daemon that never answers is asked 5 times after the call, then a failed line with the call's last line, nothing reported as changed" || bad "cpu_apply 99: $(cpu_apply 99) asked $(grep -c '' "$W/ca-asked")"
# The default mark, and the real branches (NODE_TEST unset) with the build user's command as a
# stand-in that prints it.
[ "$(
  eval "$(fn cpu_limit_want)"
  CPU_LIMIT=
  q_nproc() { echo 3; }
  cpu_limit_want
  q_nproc() { echo 1; }
  cpu_limit_want
)" = "2.4
1.0" ] && ok "the default CPU mark: 80 % of the CPUs (3 CPUs: 2.4), and never under 1 (1 CPU: 1.0)" || bad "default CPU mark"
ac=$(
  eval "$(fn a_cpu_limit)"
  T=
  STORE=/srv/slot
  as_user() { echo "$*"; }
  a_cpu_limit 25.6 40
  a_cpu_limit 8 off
  a_cpu_limit off off
)
[ "$ac" = "/srv/slot/bin/slot cpu-limit 25.6 --pressure 40
/srv/slot/bin/slot cpu-limit 8 --pressure off
/srv/slot/bin/slot cpu-limit off" ] && ok "the call without NODE_TEST: slot cpu-limit as the build user with the CPU mark and --pressure, or off alone" || bad "a_cpu_limit: $ac"
ac=$(
  eval "$(fn a_cpu_limit)"
  T=
  STORE=/srv/slot
  as_user() {
    echo try
    return 1
  }
  sleep() { echo "sleep $*"; }
  a_cpu_limit 8 40
  echo "exit $?"
)
[ "$(echo "$ac" | tr '\n' ' ')" = 'try sleep 1 try sleep 1 try sleep 1 try sleep 1 try exit 1 ' ] && ok "a daemon that does not take the call: 5 tries, 1 s apart, then the call has failed" || bad "a_cpu_limit tries: $ac"
qa=$(
  eval "$(fn q_cpu_limit_answer)"
  T=
  CPU_LIMIT_URL=http://127.0.0.1:7411/api/cpu-limit
  as_user() { echo "$*"; }
  q_cpu_limit_answer
)
[ "$qa" = 'curl -s -m 5 --max-filesize 65536 http://127.0.0.1:7411/api/cpu-limit' ] && ! grep -q 'limits\.json' "$P" && ok "the query without NODE_TEST: curl as the build user, 5 s at most; provision.sh reads no limits.json" || bad "q_cpu_limit_answer: $qa $(grep -n 'limits\.json' "$P" | head -2)"

# 6v. Draining: `apply --drain` makes a pending restart happen on a node that never has 0 jobs.
# The daemon alone gets SIGTERM: it takes no new job, waits for the running and queued ones and
# exits, and systemd starts the unit again (Restart=always) with the unit file and the groups of
# that moment. slot-node.service runs a step as root before each start (ExecStartPre, the mode
# start-pre): it restarts a sccache-node whose restart waits, and takes the mark of the drain and
# its bound away. The bound is a transient timer (slot-node-drain) that runs the mode
# drain-bound after DRAIN_MAX: a daemon process that is still the one that got the signal is
# restarted the hard way. Under NODE_TEST=1 the mode start-pre stands for a start of the unit by
# systemd: the step, then the new process. The mark is under /run/slot-node; the clock is the
# file now.
conf
echo 0 >"$RR/.state/jobs"
run "$RR" apply
run "$RR" check
[ $? -eq 0 ] && ok "before the drains: 0 drift" || bad "6v starts with drift: $(cat "$W/out")"
dmark=$RR/run/slot-node/drain
smark=$RR/run/slot-node/sccache-restart
dtimer=$RR/.state/drain-timer
why='the process has the core limit unlimited, not 0'
drun() { NODE_TEST=1 NODE_ROOT=$RR sh "$P" apply "$W/node.conf" --drain >"$W/out" 2>"$W/err"; }
# sysd MODE: what systemd runs on the node, the step before a start or the bound of a drain.
sysd() { NODE_TEST=1 NODE_ROOT=$RR sh "$P" "$1" "$W/node.conf" >"$W/out" 2>"$W/err"; }
sys_calls() { grep 'systemctl\|systemd-run' "$RR/.calls" | tr '\n' ','; }
pid_of() { cat "$RR/.state/mainpid/slot-node.service" 2>/dev/null; }
utc() { date -u -d "@$1" +%Y-%m-%dT%H:%M:%SZ; }
# stale UNIT...: the process of each unit runs with another core limit than 0, a reason to
# restart that stays until the unit restarts.
stale() { for stale_u in "$@"; do echo unlimited >"$RR/.state/core-limit/$stale_u.service"; done; }
# Without --drain nothing changes: the pending line, no signal, no mark.
stale slot-node
echo 5 >"$RR/.state/jobs"
echo 1700000000 >"$RR/.state/now"
: >"$RR/.calls"
run "$RR" apply
st=$?
[ $st -eq 0 ] && grep -qx "pending: running: slot-node restarts when no job runs (5 now): $why" "$W/err" && [ -z "$(sys_calls)" ] && [ ! -e "$dmark" ] && ! grep -q '^draining' "$W/out" && ok "apply without --drain, jobs running: the pending line as before, no signal, no mark" || bad "apply without --drain: exit $st $(sys_calls) $(cat "$W/out" "$W/err")"
# With --drain: the bound is armed, then the daemon alone gets SIGTERM. No job is signalled:
# no restart and no stop of the unit.
p0=$(pid_of)
drun
st=$?
[ $st -eq 0 ] && [ "$(grep '^draining' "$W/out")" = 'draining: slot-node: 5 job(s), restart by 2023-11-14T22:43:20Z' ] && ! grep -q 'pending: running: slot-node' "$W/err" && ok "apply --drain, 5 jobs running: one draining line with the jobs and the UTC time of the bound, exit 0, no pending line" || bad "apply --drain: exit $st $(cat "$W/out" "$W/err")"
[ "$(sys_calls)" = 'systemctl daemon-reload,systemd-run --unit=slot-node-drain --on-active=1800 provision.sh drain-bound,systemctl kill --kill-whom=main --signal=TERM slot-node.service,' ] && ok "the calls of a drain, in order: daemon-reload, the bound armed for DRAIN_MAX, SIGTERM to the main process only; no restart and no stop of the unit" || bad "drain calls: $(sys_calls)"
[ -s "$dmark" ] && [ -e "$dtimer" ] && [ -n "$p0" ] && [ "$(pid_of)" = "$p0" ] && [ -e "$RR/.state/active/slot-node.service" ] && ok "the mark of the drain is under /run/slot-node, the bound is armed, and the daemon process is the same and runs on" || bad "drain state: $(ls "$RR/run/slot-node" 2>&1) '$p0' '$(pid_of)'"
# check during the drain: one drift line for it, in place of the unit's own.
echo 3 >"$RR/.state/jobs"
run "$RR" check
st=$?
[ $st -eq 1 ] && grep -qx "drift: drain: slot-node.service drains for its restart ($why): 3 job(s) left, restart by 2023-11-14T22:43:20Z" "$W/out" && grep -qx '1 drift' "$W/out" && ! grep -q '^drift: running' "$W/out" && ok "check during a drain: one drift line with the reason, the jobs left and the time, exit 1" || bad "check during a drain: exit $st $(cat "$W/out")"
crun --canary
grep -qx 'canary: not tested while slot-node.service drains: core, cpu-limit' "$W/out" && ! grep -q '^canary: ok: core\|^canary: ok: cpu-limit' "$W/out" && grep -q '^canary: ok: role' "$W/out" && grep -qx '1 drift' "$W/out" && ok "check --canary during a drain: no ok line for the core limit and the load, one line says that they were not tested; the role's canary runs" || bad "canary during a drain: $(cat "$W/out")"
# A second apply during the drain, with --drain or without: the line again, nothing else.
echo 1700000600 >"$RR/.state/now"
echo 2 >"$RR/.state/jobs"
: >"$RR/.calls"
drun
st=$?
[ $st -eq 0 ] && [ "$(grep '^draining' "$W/out")" = 'draining: slot-node: 2 job(s), restart by 2023-11-14T22:43:20Z' ] && [ -z "$(sys_calls)" ] && [ "$(pid_of)" = "$p0" ] && ok "a second apply --drain during the drain: the line again with the jobs left and the same time; no second signal, no second bound, no restart" || bad "second apply --drain: exit $st $(sys_calls) $(cat "$W/out" "$W/err")"
run "$RR" apply
st=$?
[ $st -eq 0 ] && [ "$(grep '^draining' "$W/out")" = 'draining: slot-node: 2 job(s), restart by 2023-11-14T22:43:20Z' ] && [ -z "$(sys_calls)" ] && ! grep -q 'pending: running: slot-node' "$W/err" && ok "apply without --drain during a drain: the draining line, no signal, no restart" || bad "apply during a drain: exit $st $(sys_calls) $(cat "$W/out" "$W/err")"
# The bound, with the daemon process that got the signal still there: the hard restart.
: >"$RR/.calls"
sysd drain-bound
[ "$(sys_calls)" = 'systemctl restart slot-node.service,' ] && grep -qx 'slot-node: the drain reached its bound (DRAIN_MAX=30m) with 2 job(s) left: slot-node.service restarts now, and the jobs that still run end' "$W/out" && [ ! -e "$dmark" ] && [ ! -e "$dtimer" ] && [ "$(pid_of)" != "$p0" ] && ok "the bound with the same daemon process still draining: the unit restarts the hard way and a line says so; the mark is gone" || bad "bound, same process: $(sys_calls) $(cat "$W/out" "$W/err") '$p0' '$(pid_of)'"
echo 0 >"$RR/.state/jobs"
run "$RR" check
[ $? -eq 0 ] && grep -q '^0 drift$' "$W/out" && ok "after the hard restart: 0 drift" || bad "after the bound: $(cat "$W/out")"
# The daemon drains, exits, and systemd starts the unit again: the step before the start takes
# the mark and the bound away, and the bound, were it to run, does nothing.
stale slot-node
echo 4 >"$RR/.state/jobs"
echo 1700003600 >"$RR/.state/now"
drun
p1=$(pid_of)
[ "$(grep '^draining' "$W/out")" = "draining: slot-node: 4 job(s), restart by $(utc 1700005400)" ] && [ -s "$dmark" ] && ok "a later drain has its own time" || bad "second drain: $(cat "$W/out" "$W/err")"
: >"$RR/.calls"
sysd start-pre
p2=$(pid_of)
[ "$(sys_calls)" = 'systemctl stop slot-node-drain.timer,' ] && grep -qx 'slot-node: the drain is over: slot-node.service starts' "$W/out" && [ ! -e "$dmark" ] && [ ! -e "$dtimer" ] && [ "$p2" != "$p1" ] && ok "the daemon drained and systemd starts the unit again: the step before the start removes the mark and stops the bound, a new process runs; nothing restarted the unit" || bad "start after a drain: $(sys_calls) $(cat "$W/out" "$W/err") '$p1' '$p2'"
echo 0 >"$RR/.state/jobs"
run "$RR" check
[ $? -eq 0 ] && grep -q '^0 drift$' "$W/out" && ok "after the drain: 0 drift, nothing of the drain is left" || bad "after the drain: $(cat "$W/out")"
: >"$RR/.calls"
sysd drain-bound
[ ! -s "$RR/.calls" ] && [ ! -s "$W/out" ] && [ "$(pid_of)" = "$p2" ] && ok "the bound after the drain is over does nothing: no restart of the new daemon" || bad "bound after the drain: $(cat "$RR/.calls" "$W/out" "$W/err")"
# A mark that outlived its drain (a start that did not run the step): the daemon process is
# another one than the one that got the signal. The bound leaves that daemon alone.
stale slot-node
echo 4 >"$RR/.state/jobs"
drun
echo '9999@9' >"$RR/.state/mainpid/slot-node.service"
echo 0 >"$RR/.state/core-limit/slot-node.service"
run "$RR" check
st=$?
[ $st -eq 1 ] && grep -qx 'drift: drain: the mark of a drain that is over is still in /run/slot-node (apply removes it)' "$W/out" && grep -qx '1 drift' "$W/out" && ok "check names a mark whose drain is over: the daemon process is another one" || bad "mark of a drain that is over: exit $st $(cat "$W/out")"
: >"$RR/.calls"
sysd drain-bound
! grep -q 'systemctl restart\|systemctl kill' "$RR/.calls" && [ ! -e "$dmark" ] && [ "$(pid_of)" = '9999@9' ] && ok "the bound with another daemon process than the one that got the signal: no restart, the mark is removed" || bad "bound, new process: $(cat "$RR/.calls" "$W/out" "$W/err")"
stale slot-node
drun
echo '9998@8' >"$RR/.state/mainpid/slot-node.service"
echo 0 >"$RR/.state/core-limit/slot-node.service"
: >"$RR/.calls"
run "$RR" apply
st=$?
[ $st -eq 0 ] && grep -qx 'changed: drain: the mark of a drain that is over removed' "$W/out" && [ ! -e "$dmark" ] && [ ! -e "$dtimer" ] && [ "$(sys_calls)" = 'systemctl stop slot-node-drain.timer,' ] && ok "apply removes a mark whose drain is over and stops its bound; the daemon is not signalled" || bad "apply with a mark that is over: exit $st $(sys_calls) $(cat "$W/out" "$W/err")"
# A daemon that drained and exited must not stay down: at the bound a unit that is not active
# is started.
stale slot-node
drun
rm -f "$RR/.state/active/slot-node.service"
sysd drain-bound
[ -e "$RR/.state/active/slot-node.service" ] && grep -qx 'slot-node: slot-node.service is not active after its drain: starting it' "$W/out" && [ ! -e "$dmark" ] && ok "the bound finds the unit not active after its drain: it starts it and says so" || bad "bound, unit down: $(cat "$W/out" "$W/err")"
echo 0 >"$RR/.state/jobs"
run "$RR" check
[ $? -eq 0 ] || bad "after the start by the bound: $(cat "$W/out")"
# sccache-node with a restart that waits: it restarts in the gap, after the old daemon exited
# and before the new one starts, also when systemd starts the unit by itself.
stale slot-node sccache-node
echo 6 >"$RR/.state/jobs"
: >"$RR/.calls"
drun
[ "$(grep -c '^draining' "$W/out")" -eq 1 ] && grep -qx "pending: running: sccache-node restarts when slot-node does, at the end of its drain: $why" "$W/err" && [ "$(cat "$smark" 2>&1)" = "$why" ] && ! grep -q 'systemctl restart' "$RR/.calls" && ok "a drain with a pending restart of sccache-node: one draining line, a pending line for sccache-node, its mark under /run/slot-node, no restart now" || bad "drain with sccache-node: $(sys_calls) $(cat "$W/out" "$W/err")"
: >"$RR/.calls"
sysd start-pre
[ "$(sys_calls)" = 'systemctl restart sccache-node.service,systemctl stop slot-node-drain.timer,' ] && grep -qx "slot-node: sccache-node.service restarted before slot-node.service starts: $why" "$W/out" && [ ! -e "$smark" ] && [ ! -e "$dmark" ] && ok "the step before the start of slot-node restarts sccache-node in the gap and removes both marks" || bad "gap: $(sys_calls) $(cat "$W/out" "$W/err")"
echo 0 >"$RR/.state/jobs"
run "$RR" check
[ $? -eq 0 ] && grep -q '^0 drift$' "$W/out" && ok "after the gap: both units run from their files, 0 drift" || bad "after the gap: $(cat "$W/out")"
# A unit file of slot-node.service without the step: nothing is done before the start.
stale slot-node sccache-node
echo 6 >"$RR/.state/jobs"
drun
sed -i '/^ExecStartPre=/d' "$RR/etc/systemd/system/slot-node.service"
: >"$RR/.calls"
sysd start-pre
[ -e "$smark" ] && [ -e "$dmark" ] && ! grep -q 'systemctl restart sccache-node' "$RR/.calls" && ok "NODE_TEST=1 runs the step only for a unit file that names it (ExecStartPre)" || bad "start without ExecStartPre: $(sys_calls) $(ls "$RR/run/slot-node")"
echo 0 >"$RR/.state/jobs"
run "$RR" apply
run "$RR" apply
run "$RR" check
[ $? -eq 0 ] && [ ! -e "$smark" ] && [ ! -e "$dmark" ] && ok "apply writes the step back, restarts both units and removes the marks: 0 drift" || bad "after the unit file without the step: $(cat "$W/out") $(ls "$RR/run/slot-node" 2>&1)"
# A pending restart of sccache-node alone drains slot-node too: the gap is its only moment.
stale sccache-node
echo 2 >"$RR/.state/jobs"
: >"$RR/.calls"
drun
[ "$(grep '^draining' "$W/out")" = "draining: slot-node: 2 job(s), restart by $(utc 1700005400)" ] && grep -q 'systemctl kill --kill-whom=main --signal=TERM slot-node.service' "$RR/.calls" && [ -e "$smark" ] && ! grep -q 'systemctl restart' "$RR/.calls" && ok "a pending restart of sccache-node alone: apply --drain drains slot-node for it" || bad "sccache-node alone: $(sys_calls) $(cat "$W/out" "$W/err")"
run "$RR" check
grep -qx "drift: drain: slot-node.service drains for its restart (sccache-node.service restarts: $why): 2 job(s) left, restart by $(utc 1700005400)" "$W/out" && grep -qx '1 drift' "$W/out" && ok "check names the reason of that drain" || bad "check, sccache-node alone: $(cat "$W/out")"
sysd start-pre
echo 0 >"$RR/.state/jobs"
run "$RR" check
[ $? -eq 0 ] || bad "after the drain for sccache-node: $(cat "$W/out")"
# No job: --drain restarts at once, as apply does.
stale slot-node
: >"$RR/.calls"
drun
[ "$(sys_calls)" = 'systemctl restart slot-node.service,' ] && grep -qx "changed: running: slot-node.service restarted: $why" "$W/out" && ! grep -q '^draining' "$W/out" && [ ! -e "$dmark" ] && ok "apply --drain with no job: the unit restarts at once, nothing drains" || bad "drain with no job: $(sys_calls) $(cat "$W/out" "$W/err")"
# A bound that cannot be armed: no signal. A signal that fails: no mark and no bound stay.
stale slot-node
echo 4 >"$RR/.state/jobs"
: >"$RR/.state-drain-arm-fails"
: >"$RR/.calls"
drun
st=$?
[ $st -eq 1 ] && grep -qx 'failed: drain: the bound could not be armed (systemd-run --unit=slot-node-drain): slot-node.service was not signalled' "$W/err" && ! grep -q 'systemctl kill' "$RR/.calls" && [ ! -e "$dmark" ] && ! grep -q '^draining' "$W/out" && ok "a bound that cannot be armed: failed line, exit 1, the daemon is not signalled, no mark" || bad "arm fails: exit $st $(sys_calls) $(cat "$W/out" "$W/err")"
rm -f "$RR/.state-drain-arm-fails"
: >"$RR/.state-kill-fails"
: >"$RR/.calls"
drun
st=$?
[ $st -eq 1 ] && grep -qx 'failed: drain: systemctl kill --kill-whom=main --signal=TERM slot-node.service failed: nothing drains' "$W/err" && [ ! -e "$dmark" ] && [ ! -e "$dtimer" ] && grep -q 'systemctl stop slot-node-drain.timer' "$RR/.calls" && ! grep -q '^draining' "$W/out" && ok "a signal that fails: failed line, exit 1, the mark is removed and the bound stopped" || bad "kill fails: exit $st $(sys_calls) $(cat "$W/out" "$W/err")"
rm -f "$RR/.state-kill-fails"
# DRAIN_MAX names the bound; a daemon that does not say how many jobs run drains too.
echo 'DRAIN_MAX=45m' >>"$W/node.conf"
: >"$RR/.state/jobs"
: >"$RR/.calls"
drun
[ "$(grep '^draining' "$W/out")" = "draining: slot-node: ? job(s), restart by $(utc 1700006300)" ] && grep -qx 'systemd-run --unit=slot-node-drain --on-active=2700 provision.sh drain-bound' "$RR/.calls" && ok "DRAIN_MAX=45m: the bound is armed for 2700 s; an unknown number of jobs is a ?" || bad "DRAIN_MAX=45m: $(sys_calls) $(cat "$W/out" "$W/err")"
: >"$RR/.state-no-metrics"
run "$RR" check
grep -qx "drift: drain: slot-node.service drains for its restart ($why): ? job(s) left, restart by $(utc 1700006300)" "$W/out" && ok "check during a drain whose daemon gives no job count: a ? for the jobs left" || bad "check without a job count: $(cat "$W/out")"
rm -f "$RR/.state-no-metrics"
sysd drain-bound
grep -qx 'slot-node: the drain reached its bound (DRAIN_MAX=45m) with ? job(s) left: slot-node.service restarts now, and the jobs that still run end' "$W/out" || bad "bound with DRAIN_MAX=45m: $(cat "$W/out" "$W/err")"
conf
echo 0 >"$RR/.state/jobs"
rm -f "$RR/.state/now"
run "$RR" apply
run "$RR" check
[ $? -eq 0 ] && grep -q '^0 drift$' "$W/out" && ok "after the drains: 0 drift" || bad "6v ends with drift: $(cat "$W/out")"
# The option: apply takes --drain, alone or with --open; check does not; a bundle carries it.
sh "$P" bundle "$W/node.conf" apply --drain >"$W/bundle-drain.sh"
grep -qx 'sh "$d/provision.sh" apply "$d/node.conf" --drain' "$W/bundle-drain.sh" && sh -n "$W/bundle-drain.sh" && ok "an apply bundle carries --drain" || bad "bundle --drain: $(tail -4 "$W/bundle-drain.sh" | head -1)"
sh "$P" bundle "$W/node.conf" apply --open --drain >"$W/bundle-drain.sh"
grep -qx 'sh "$d/provision.sh" apply "$d/node.conf" --open --drain' "$W/bundle-drain.sh" && ok "an apply bundle carries --open and --drain together" || bad "bundle --open --drain: $(tail -4 "$W/bundle-drain.sh" | head -1)"
sh "$P" bundle "$W/node.conf" check --drain >"$W/out" 2>"$W/err"
[ $? -eq 2 ] && ok "a check bundle with --drain is refused" || bad "bundle check --drain: $(head -2 "$W/out")"
NODE_TEST=1 NODE_ROOT=$RR sh "$P" check "$W/node.conf" --drain >"$W/out" 2>"$W/err"
[ $? -eq 2 ] && ok "check --drain is refused" || bad "check --drain ran: $(cat "$W/out")"
NODE_TEST=1 NODE_ROOT=$RR sh "$P" apply "$W/node.conf" --drian >"$W/out" 2>"$W/err"
[ $? -eq 2 ] && grep -q 'provision.sh apply CONF \[--open\] \[--drain\]' "$W/err" && ok "apply with a misspelled --drain is refused, and the usage names --drain" || bad "apply --drian ran: $(cat "$W/out" "$W/err" | head -3)"
: >"$RR/.calls"
NODE_TEST=1 NODE_ROOT=$RR sh "$P" apply "$W/node.conf" --drain --open >"$W/out" 2>"$W/err"
[ $? -eq 0 ] && ok "apply takes --drain and --open in either order" || bad "apply --drain --open: $(cat "$W/err")"
run "$RR" apply
# The real branches (NODE_TEST unset), each alone with stand-ins for systemctl and systemd-run.
mkdir -p "$W/sdbin"
printf '#!/bin/sh\necho "systemd-run $*"\n' >"$W/sdbin/systemd-run"
chmod 755 "$W/sdbin/systemd-run"
rb=$(
  eval "$(fn a_signal_main a_drain_arm a_drain_disarm)"
  T= LIBEXEC=/usr/local/libexec DRAIN_UNIT=slot-node-drain PATH=$W/sdbin:$PATH
  systemctl() { echo "systemctl $*"; }
  a_signal_main slot-node.service
  a_drain_arm 1800
  a_drain_disarm
)
[ "$rb" = "systemctl kill --kill-whom=main --signal=TERM slot-node.service
systemctl stop slot-node-drain.timer
systemd-run --quiet --collect --unit=slot-node-drain --on-active=1800 --timer-property=AccuracySec=1s /usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin /bin/sh /usr/local/libexec/slot-node/provision.sh drain-bound /etc/slot-node/node.conf
systemctl stop slot-node-drain.timer" ] && ok "without NODE_TEST: the signal goes to the main process only (--kill-whom=main), and the bound is a transient timer named slot-node-drain that runs the mode drain-bound with a clean environment" || bad "real drain calls: $rb"
mi=$(
  eval "$(fn q_main_id)"
  T=
  systemctl() { printf 'MainPID=4711\nExecMainStartTimestampMonotonic=88123456\n'; }
  q_main_id slot-node.service
  systemctl() { printf 'ExecMainStartTimestampMonotonic=99000000\nMainPID=4711\n'; }
  q_main_id slot-node.service
  systemctl() { printf 'MainPID=0\nExecMainStartTimestampMonotonic=0\n'; }
  q_main_id slot-node.service
  echo end
)
[ "$mi" = "4711@88123456
4711@99000000
end" ] && ok "the daemon process is named by its main PID and its start time: the same PID started later is another process; no process, no name" || bad "q_main_id: $mi"
jl=$(
  eval "$(fn q_jobs_left)"
  T= METRICS_URL=http://127.0.0.1:7411/metrics
  as_user() { echo "$*" >"$W/jl-cmd"; printf '# TYPE slot_jobs_queued gauge\nslot_jobs_queued{toolchain="rust"} 2\n# TYPE slot_jobs_running gauge\nslot_jobs_running{toolchain="go"} 1\nslot_jobs_running{toolchain="rust"} 4\nslot_jobs_finished_total{toolchain="rust",result="ok"} 9\n'; }
  q_jobs_left
  as_user() { printf '# TYPE slot_jobs_queued gauge\n# TYPE slot_jobs_running gauge\n'; }
  q_jobs_left
  as_user() { echo 'not found'; }
  q_jobs_left
  as_user() { :; }
  q_jobs_left
  echo end
)
[ "$jl" = "7
0
end" ] && [ "$(cat "$W/jl-cmd")" = 'curl -s -m 5 --max-filesize 1048576 http://127.0.0.1:7411/metrics' ] && ok "the jobs left during a drain: running and queued jobs of the daemon's /metrics, asked as the build user, 5 s at most; no answer is no number" || bad "q_jobs_left: $jl $(cat "$W/jl-cmd")"

# 6x. One `apply --drain` is enough for the core pattern. The pattern is loaded only once
# sccache-node and slot-node run with the soft core limit 0 (6s), and after `apply --drain` both
# restart later, by themselves, when no apply runs: the step before the start of slot-node.service
# (start-pre) loads /etc/sysctl.d/60-slot-core.conf at the end of a drain, under the condition
# an apply checks. A root of its own: a node provisioned before the core limit, with 1 job.
conf
RX=$W/r6x
mkdir -p "$RX"
echo 8 >"$RX/.state-nproc"
xpipe='|/usr/lib/systemd/systemd-coredump %P %u %g %s %t %c %h'
xsys=/etc/sysctl.d/60-slot-core.conf
xrun() { NODE_TEST=1 NODE_ROOT=$RX sh "$P" "$@" >"$W/out" 2>"$W/err"; }
xcalls() { grep 'systemctl restart\|sysctl' "$RX/.calls" | tr '\n' ','; }
# xold [UNIT...]: the node as it was before the core limit, with the pipe as its pattern: no
# limit in the unit files named (default: both), every process with the limit unlimited, 1 job.
xold() {
  [ $# -gt 0 ] || set -- slot-node sccache-node
  rm -f "$RX/.state-core-stays" "$RX/.state-start-fails-sccache-node.service"
  mkdir -p "$RX/.state"
  echo 0 >"$RX/.state/jobs"
  xrun apply "$W/node.conf"
  echo "$xpipe" >"$RX/.state/core-pattern"
  for xu in "$@"; do sed -i '/^LimitCORE=/d' "$RX/etc/systemd/system/$xu.service"; done
  for xu in slot-node sccache-node; do echo unlimited >"$RX/.state/core-limit/$xu.service"; done
  echo 1 >"$RX/.state/jobs"
  echo 1700000000 >"$RX/.state/now"
  : >"$RX/.state/journal"
  : >"$RX/.calls"
}
xold
xrun apply "$W/node.conf" --drain
st=$?
[ $st -eq 0 ] && grep -q '^draining: slot-node: 1 job(s), restart by ' "$W/out" && [ -z "$(xcalls)" ] && [ "$(cat "$RX/.state/core-pattern")" = "$xpipe" ] && ok "apply --drain with a job running: the drain starts, no unit restarts, the pattern is not loaded" || bad "6x apply --drain: exit $st $(xcalls) $(cat "$W/out" "$W/err")"
grep -qx "pending: core: kernel.core_pattern stays '$xpipe' while sccache-node.service runs with the core limit unlimited (under the pattern core its processes write core files): the start of slot-node.service at the end of its drain loads $xsys" "$W/err" && ok "apply --drain: the pending line names the start after the drain, not a second apply" || bad "6x pending line: $(grep 'pending: core' "$W/err")"
echo 0 >"$RX/.state/jobs"
: >"$RX/.calls"
xrun start-pre "$W/node.conf"
[ "$(xcalls)" = "systemctl restart sccache-node.service,sysctl -q -p $xsys," ] && [ "$(cat "$RX/.state/core-pattern")" = core ] && ok "the start after the drain: sccache-node restarts, then the sysctl file is loaded and the pattern is core" || bad "6x start-pre: $(xcalls) pattern '$(cat "$RX/.state/core-pattern")' $(cat "$W/out" "$W/err")"
grep -qx "slot-node: kernel.core_pattern is 'core' (sysctl -p $xsys)" "$W/out" && ok "the start after the drain says in the journal that it loaded the pattern" || bad "6x journal: $(cat "$W/out")"
: >"$RX/.calls"
xrun check "$W/node.conf"
st=$?
[ $st -eq 0 ] && grep -qx '0 drift' "$W/out" && ok "after one apply --drain and the restart it started: 0 drift, the pattern is no pipe" || bad "6x check after the drain: exit $st $(grep '^drift' "$W/out" | tr '\n' ' ')"
xrun apply "$W/node.conf"
grep -qx '0 changed' "$W/out" && [ -z "$(xcalls)" ] && ok "a second apply has nothing left to do" || bad "6x second apply: $(xcalls) $(cat "$W/out" "$W/err")"
# The bound of the drain restarts the unit the hard way: the same step, the same load.
xold
xrun apply "$W/node.conf" --drain
: >"$RX/.calls"
xrun drain-bound "$W/node.conf"
[ "$(xcalls)" = "systemctl restart slot-node.service,systemctl restart sccache-node.service,sysctl -q -p $xsys," ] && [ "$(cat "$RX/.state/core-pattern")" = core ] && grep -qx "slot-node: kernel.core_pattern is 'core' (sysctl -p $xsys)" "$RX/.state/journal" && ok "the bound of a drain: the restart runs the same step, and the pattern is loaded" || bad "6x bound: $(xcalls) pattern '$(cat "$RX/.state/core-pattern")' $(cat "$RX/.state/journal" 2>&1)"
# sccache-node does not restart: its process keeps the limit unlimited, and the pattern stays.
xold
xrun apply "$W/node.conf" --drain
: >"$RX/.state-start-fails-sccache-node.service"
echo 0 >"$RX/.state/jobs"
: >"$RX/.calls"
xrun start-pre "$W/node.conf"
! grep -q sysctl "$RX/.calls" && [ "$(cat "$RX/.state/core-pattern")" = "$xpipe" ] && grep -qx "slot-node: kernel.core_pattern stays '$xpipe' while sccache-node.service runs with the core limit unlimited: the next apply loads $xsys" "$W/out" && ok "sccache-node did not restart: the pattern stays a pipe, and one line says why" || bad "6x sccache stays: $(xcalls) $(cat "$W/out" "$W/err")"
rm -f "$RX/.state-start-fails-sccache-node.service"
xrun apply "$W/node.conf"
[ "$(cat "$RX/.state/core-pattern")" = core ] && grep -q '^changed: core: kernel.core_pattern is .core. ' "$W/out" && ok "the next apply restarts sccache-node and loads the pattern, as before" || bad "6x apply after a failed restart: $(cat "$W/out" "$W/err")"
# The unit file of slot-node.service has no core limit at the start (taken out after the apply):
# its next process would write core files under the pattern core, so the pattern stays.
xold
xrun apply "$W/node.conf" --drain
sed -i '/^LimitCORE=/d' "$RX/etc/systemd/system/slot-node.service"
: >"$RX/.calls"
xrun start-pre "$W/node.conf"
! grep -q sysctl "$RX/.calls" && [ "$(cat "$RX/.state/core-pattern")" = "$xpipe" ] && grep -qx "slot-node: kernel.core_pattern stays '$xpipe' while slot-node.service starts with the core limit unlimited: the next apply loads $xsys" "$W/out" && ok "slot-node would start with another core limit than 0: the pattern stays a pipe" || bad "6x unit without a limit: $(xcalls) $(cat "$W/out" "$W/err")"
# A load that does not change the pattern is named.
xold
xrun apply "$W/node.conf" --drain
: >"$RX/.state-core-stays"
xrun start-pre "$W/node.conf"
grep -qx "slot-node: kernel.core_pattern is still '$xpipe' after sysctl -p $xsys" "$W/out" && ok "a sysctl load that leaves the pipe is named in the journal" || bad "6x pattern stays after the load: $(cat "$W/out" "$W/err")"
rm -f "$RX/.state-core-stays"
# No sysctl file (the owner's pattern was kept, then became a pipe): nothing is loaded.
xold
xrun apply "$W/node.conf" --drain
rm -f "$RX$xsys"
: >"$RX/.calls"
xrun start-pre "$W/node.conf"
! grep -q sysctl "$RX/.calls" && ! grep -q 'core_pattern' "$W/out" && ok "no sysctl file: the start after a drain loads nothing" || bad "6x no sysctl file: $(xcalls) $(cat "$W/out")"
# A pattern that is no pipe is left alone, and a start that ends no drain loads nothing.
xold
xrun apply "$W/node.conf" --drain
echo '/var/crash/core.%e.%p' >"$RX/.state/core-pattern"
: >"$RX/.calls"
xrun start-pre "$W/node.conf"
! grep -q sysctl "$RX/.calls" && [ "$(cat "$RX/.state/core-pattern")" = '/var/crash/core.%e.%p' ] && ok "a pattern that is no pipe: the start after a drain leaves it alone" || bad "6x owner's pattern: $(xcalls) $(cat "$RX/.state/core-pattern")"
echo "$xpipe" >"$RX/.state/core-pattern"
: >"$RX/.calls"
xrun start-pre "$W/node.conf"
! grep -q sysctl "$RX/.calls" && [ "$(cat "$RX/.state/core-pattern")" = "$xpipe" ] && ok "a start of the unit that ends no drain loads nothing: the pattern is an apply's, or a drain's end" || bad "6x start without a drain: $(xcalls) $(cat "$W/out")"
# What the next process of a unit gets is asked from systemd: the unit's LimitCORESoft.
nl=$(
  eval "$(sed -n '/^q_core_limit_next() {$/,/^}$/p' "$P")"
  T=
  systemctl() { echo "$*" >"$W/nl-cmd"; echo 0; }
  q_core_limit_next slot-node.service
  systemctl() { echo infinity; }
  q_core_limit_next slot-node.service
  systemctl() { return 1; }
  q_core_limit_next slot-node.service
  echo end
)
[ "$nl" = "0
infinity
end" ] && [ "$(cat "$W/nl-cmd" 2>/dev/null)" = 'show -p LimitCORESoft --value slot-node.service' ] && ok "the limit of a unit's next process is systemd's LimitCORESoft of the unit; no answer is no 0" || bad "q_core_limit_next: $nl / $(cat "$W/nl-cmd" 2>&1)"

# 6w. The run-time limit of the node's pool: `slot run-limit` with RUN_LIMIT (10m) and
# RUN_LIMIT_MAX (30m, the most a job names with `slot --timeout`), set once slot-node runs
# (section 2). check asks the daemon (GET /api/run-limit as the build user, 5 s at most), which
# answers in seconds: the config's durations are compared as seconds. Under NODE_TEST=1 the
# answer is made from the files run-limit and run-limit-max.
[ "$(cat "$RR/.state/run-limit" "$RR/.state/run-limit-max" 2>&1 | tr '\n' ' ')" = '600 1800 ' ] && ok "the pool of this root has the run-time limit 600 s and the most a job names 1800 s" || bad "run limit of the root: $(cat "$RR/.state/run-limit" "$RR/.state/run-limit-max" 2>&1)"
for same in 'RUN_LIMIT=600' 'RUN_LIMIT=600s' 'RUN_LIMIT_MAX=1800' 'RUN_LIMIT_MAX=1800s'; do
  conf
  echo "$same" >>"$W/node.conf"
  : >"$RR/.calls"
  run "$RR" apply
  run "$RR" check
  [ $? -eq 0 ] && ! grep -q 'run-limit' "$RR/.calls" && ok "$same is the limit the pool has, in another form: no drift, no call" || bad "$same: $(cat "$W/out") $(grep run-limit "$RR/.calls")"
done
conf
printf 'RUN_LIMIT=20m\nRUN_LIMIT_MAX=1h\n' >>"$W/node.conf"
run "$RR" check
st=$?
[ $st -eq 1 ] && grep -qx "drift: run-limit: the pool ends a job that runs longer than '600' s, and a job names up to '1800' s, want '1200' s (20m) and '3600' s (1h)" "$W/out" && ok "RUN_LIMIT and RUN_LIMIT_MAX name another limit: check names the difference in seconds" || bad "run limit drift: exit $st $(cat "$W/out")"
: >"$RR/.calls"
run "$RR" apply
st=$?
[ $st -eq 0 ] && grep -qx 'slot run-limit 20m --max 1h' "$RR/.calls" && grep -qx 'changed: run-limit: a job that runs longer than 20m is ended (exit 124), and a job names up to 1h (slot run-limit 20m --max 1h)' "$W/out" && [ "$(cat "$RR/.state/run-limit" "$RR/.state/run-limit-max" | tr '\n' ' ')" = '1200 3600 ' ] && ok "apply sets the limit of the config" || bad "run limit apply: exit $st $(grep run-limit "$RR/.calls" "$W/out" "$W/err")"
# Only the most a job names differs: drift too, before the call and after it.
conf
printf 'RUN_LIMIT=20m\nRUN_LIMIT_MAX=30m\n' >>"$W/node.conf"
run "$RR" check
grep -qx "drift: run-limit: the pool ends a job that runs longer than '1200' s, and a job names up to '3600' s, want '1200' s (20m) and '1800' s (30m)" "$W/out" && ok "only the most a job names differs: drift" || bad "max differs: $(cat "$W/out")"
: >"$RR/.state-run-limit-stays"
run "$RR" apply
st=$?
[ $st -eq 1 ] && grep -qx "failed: run-limit: after slot run-limit 20m --max 30m the pool ends a job that runs longer than '1200' s, and a job names up to '3600' s" "$W/err" && ! grep -q '^changed: run-limit' "$W/out" && ok "a limit the pool does not have after the call: failed line, exit 1, not reported as changed" || bad "run limit stays: exit $st $(grep run-limit "$W/out" "$W/err")"
rm -f "$RR/.state-run-limit-stays"
conf
echo 'RUN_LIMIT=off' >>"$W/node.conf"
run "$RR" check
grep -qx "drift: run-limit: the pool ends a job that runs longer than '1200' s, and a job names up to '3600' s, want 'off' and '1800' s (30m)" "$W/out" && ok "RUN_LIMIT=off: a limit the pool has is drift" || bad "off: $(cat "$W/out")"
: >"$RR/.calls"
run "$RR" apply
st=$?
[ $st -eq 0 ] && grep -qx 'slot run-limit off --max 30m' "$RR/.calls" && grep -qx 'changed: run-limit: no run-time limit, and a job names up to 30m (slot run-limit off --max 30m)' "$W/out" && [ ! -e "$RR/.state/run-limit" ] && [ "$(cat "$RR/.state/run-limit-max")" = 1800 ] && ok "RUN_LIMIT=off: apply turns the limit off and keeps the most a job names" || bad "off apply: exit $st $(grep run-limit "$RR/.calls" "$W/out" "$W/err")"
run "$RR" check
[ $? -eq 0 ] && ok "RUN_LIMIT=off: 0 drift after apply" || bad "off, after apply: $(cat "$W/out")"
conf
run "$RR" check
grep -qx "drift: run-limit: the pool has no run-time limit, and a job names up to '1800' s, want '600' s (10m) and '1800' s (30m)" "$W/out" && ok "a pool without a run-time limit drifts from the default" || bad "no limit: $(cat "$W/out")"
# A daemon that does not run gives no answer: check says so, and apply does not call it.
rm -f "$RR/.state/active/slot-node.service"
: >"$RR/.state-start-fails-slot-node.service"
run "$RR" check
grep -qx "drift: run-limit: the daemon of the pool gives no answer about its run-time limit (GET http://127.0.0.1:7411/api/run-limit as $me, 5 s at most), want '600' s (10m) and '1800' s (30m)" "$W/out" && ok "with slot-node.service not active no daemon answers: check says so and names the limit it wants" || bad "not active, check: $(grep run-limit "$W/out")"
: >"$RR/.calls"
run "$RR" apply
st=$?
[ $st -eq 1 ] && grep -qx 'failed: run-limit: slot-node.service is not active: the limit was not set' "$W/err" && ! grep -q 'run-limit' "$RR/.calls" && ok "a slot-node.service that does not start: a failed line for the run-time limit, no slot run-limit call" || bad "not active, apply: exit $st $(cat "$W/err" "$RR/.calls")"
rm -f "$RR/.state-start-fails-slot-node.service"
run "$RR" apply
st=$?
[ $st -eq 0 ] && grep -qx 'slot run-limit 10m --max 30m' "$RR/.calls" && [ "$(cat "$RR/.state/run-limit" 2>&1)" = 600 ] && ok "once slot-node.service starts, apply sets the run-time limit" || bad "limit after the start: exit $st $(cat "$RR/.calls" "$W/out" "$W/err")"
run "$RR" check
[ $? -eq 0 ] || bad "6w ends with drift: $(cat "$W/out")"
# p_run_limit alone in check mode, wanting 10m and 30m, on the answer $1.
rl_fn=$(fn is_decimal cpu_mark run_limit_read run_limit_holds run_limit_text p_run_limit)
run_check() {
  (
    . "$here/conf.sh"
    eval "$rl_fn"
    mode=check U=builder RUN_LIMIT=${2:-10m} RUN_LIMIT_MAX=30m RUN_LIMIT_URL=http://127.0.0.1:7411/api/run-limit ra=$1
    drift() { echo "drift: $1: $2"; }
    fixing() { false; }
    q_run_limit_answer() { printf '%s\n' "$ra"; }
    p_run_limit 2>&1
  )
}
[ -z "$(run_check '{"run_limit_s":600,"run_limit_max_s":1800}')" ] && [ -z "$(run_check '{"run_limit_max_s":1800,"run_limit_s":null}' off)" ] && [ "$(run_check '{"run_limit_s":null,"run_limit_max_s":3600}')" = "drift: run-limit: the pool has no run-time limit, and a job names up to '3600' s, want '600' s (10m) and '1800' s (30m)" ] && ok "the daemon's answer in seconds: 600 and 1800 are 10m and 30m, null is off; the answer of a daemon without a limit is drift" || bad "run limit answers: '$(run_check '{"run_limit_s":600,"run_limit_max_s":1800}')' '$(run_check '{"run_limit_max_s":1800,"run_limit_s":null}' off)' $(run_check '{"run_limit_s":null,"run_limit_max_s":3600}')"
[ "$(run_check '{"run_limit_s":601,"run_limit_max_s":1800}')" = "drift: run-limit: the pool ends a job that runs longer than '601' s, and a job names up to '1800' s, want '600' s (10m) and '1800' s (30m)" ] && [ "$(run_check '{"run_limit_s":600,"run_limit_max_s":1801}')" = "drift: run-limit: the pool ends a job that runs longer than '600' s, and a job names up to '1801' s, want '600' s (10m) and '1800' s (30m)" ] && [ -n "$(run_check '{"run_limit_s":600,"run_limit_max_s":1800}' off)" ] && [ -n "$(run_check '{"run_limit_s":10,"run_limit_max_s":1800}')" ] && ok "one second more in the limit or in the most a job names is drift; a limit where off is wanted too; 10 s is not 10m" || bad "run limit compare: $(run_check '{"run_limit_s":601,"run_limit_max_s":1800}') / $(run_check '{"run_limit_s":600,"run_limit_max_s":1801}') / $(run_check '{"run_limit_s":600,"run_limit_max_s":1800}' off) / $(run_check '{"run_limit_s":10,"run_limit_max_s":1800}')"
rna="drift: run-limit: the daemon of the pool gives no answer about its run-time limit (GET http://127.0.0.1:7411/api/run-limit as builder, 5 s at most), want '600' s (10m) and '1800' s (30m)"
miss=
for a in '' '{}' 'not found' '{"run_limit_s":600}' '{"run_limit_s":600,"run_limit_max_s":null}' '{"run_limit_s":"600","run_limit_max_s":1800}' '{"run_limit_s":600.5,"run_limit_max_s":1800}' '{"run_limit_s":600,"run_limit_max_s":18e2}' '{"run_limit_s":-600,"run_limit_max_s":1800}' '{"run_limit_s":600,"run_limit_max_s":1800' '{"run_limit_s":600,"run_limit_m'; do
  [ "$(run_check "$a")" = "$rna" ] || miss="$miss '$a': $(run_check "$a");"
done
[ -z "$miss" ] && ok "an answer without both values, or with one that is no whole number of seconds (nothing, {}, a text, no max, a null max, a string, 600.5, 18e2, -600, a cut-off object), is drift: no answer" || bad "run limit answers that are none:$miss"
ar=$(
  eval "$(fn a_run_limit q_run_limit_answer)"
  T= STORE=/srv/slot RUN_LIMIT_URL=http://127.0.0.1:7411/api/run-limit
  as_user() { echo "$*"; }
  a_run_limit 10m 30m
  a_run_limit off 30m
  q_run_limit_answer
)
[ "$ar" = "/srv/slot/bin/slot run-limit 10m --max 30m
/srv/slot/bin/slot run-limit off --max 30m
curl -s -m 5 --max-filesize 65536 http://127.0.0.1:7411/api/run-limit" ] && ok "without NODE_TEST: slot run-limit as the build user with the limit (or off) and --max, and the query is curl as the build user, 5 s at most" || bad "a_run_limit: $ar"
ar=$(
  eval "$(fn a_run_limit)"
  T= STORE=/srv/slot
  as_user() {
    echo try
    return 1
  }
  sleep() { echo "sleep $*"; }
  a_run_limit 10m 30m
  echo "exit $?"
)
[ "$(echo "$ar" | tr '\n' ' ')" = 'try sleep 1 try sleep 1 try sleep 1 try sleep 1 try exit 1 ' ] && ok "a daemon that does not take the run-limit call: 5 tries, 1 s apart, then the call has failed" || bad "a_run_limit tries: $ar"

# 6u. toolchains-refresh.sh, on the workstation: both manifests are written whole or not at all.
# rsync is replaced: it copies $W/rf/node-manifest when that file exists and fails otherwise;
# toolchains.sh is a copy beside the script that prints $W/rf/local-lines; timeout is replaced by
# one that records its arguments and runs the command.
# The copy reads where its output goes before it writes the name down: dash redirects in the
# shell itself while a command with a redirection runs, so `readlink /proc/$$/fd/1 >file` reads
# the name of that file there.
RF=$W/rf
mkdir -p "$RF/bin" "$RF/lib" "$RF/home" "$RF/state/slot"
cp "$here/toolchains-refresh.sh" "$RF/lib/"
cat >"$RF/lib/toolchains.sh" <<EOF
#!/bin/sh
dst=\$(readlink /proc/\$\$/fd/1)
printf '%s\n' "\$dst" >"$RF/local.dst"
[ -f "$RF/local-lines" ] || exit 1
cat "$RF/local-lines"
EOF
cat >"$RF/bin/rsync" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >"$RF/rsync.args"
for a in "\$@"; do dst=\$a; done
printf '%s\n' "\$dst" >"$RF/rsync.dst"
[ -f "$RF/node-manifest" ] || { echo "rsync: connection unexpectedly closed" >&2; exit 23; }
cp "$RF/node-manifest" "\$dst"
while [ -e "$RF/rsync-block" ]; do sleep 1; done
EOF
cat >"$RF/bin/timeout" <<EOF
#!/bin/sh
printf '%s\n' "\$1 \$2" >"$RF/timeout.args"
shift
exec "\$@"
EOF
chmod 755 "$RF/bin/rsync" "$RF/bin/timeout"
rf() {
  HOME=$RF/home XDG_STATE_HOME=$RF/state XDG_CONFIG_HOME=${RFC:-$RF/config} PATH=$RF/bin:$PATH \
    sh "$RF/lib/toolchains-refresh.sh" "$@" >"$W/out" 2>"$W/err"
}
rf_left() { find "$RF/state/slot" -name '*.tmp*'; }
echo "node v22.23.3" >"$RF/local-lines"
echo "node v22.23.3" >"$RF/node-manifest"
rf node-1 0
rc=$?
[ $rc -eq 0 ] && [ "$(cat "$RF/state/slot/toolchains-local.txt")" = "node v22.23.3" ] &&
  [ "$(cat "$RF/state/slot/toolchains-node.txt")" = "node v22.23.3" ] && [ ! -s "$W/err" ] &&
  [ -z "$(rf_left)" ] &&
  ok "refresh: SECONDS 0 writes both manifests once and exits 0" ||
  bad "refresh once: rc=$rc $(cat "$W/err") $(rf_left)"
grep -q -- "-e ssh -F '$RF/config/slot/node/ssh_config' -o BatchMode=yes -o ConnectTimeout=10 node-1:$RF/state/slot/toolchains-node.txt " "$RF/rsync.args" &&
  ok "refresh: the node's file is read at the same path with the node ssh config, its path quoted" ||
  bad "refresh rsync: $(cat "$RF/rsync.args")"
grep -q -- ' --timeout=20 --max-size=1m ' "$RF/rsync.args" && [ "$(cat "$RF/timeout.args")" = "60 rsync" ] &&
  ok "refresh: the read of the node ends after 60 s, a silent connection after 20 s, a file over 1 MiB is not taken" ||
  bad "refresh bounds: $(cat "$RF/rsync.args") timeout: $(cat "$RF/timeout.args" 2>&1)"
# Each run writes through files of its own, so two runs never share a half-written one.
case "$(cat "$RF/rsync.dst")|$(cat "$RF/local.dst")" in
  "$RF/state/slot/toolchains-node.txt.tmp."[0-9]*"|$RF/state/slot/toolchains-local.txt.tmp."[0-9]*)
    ok "refresh: both temporary files carry the id of the run's process" ;;
  *) bad "refresh temporary names: $(cat "$RF/rsync.dst") $(cat "$RF/local.dst")" ;;
esac
# A node that does not answer keeps the node manifest that is there.
rm "$RF/node-manifest"
echo "node v22.99.0" >"$RF/local-lines"
rf node-1 0
rc=$?
[ $rc -eq 1 ] && [ "$(cat "$RF/state/slot/toolchains-node.txt")" = "node v22.23.3" ] &&
  [ "$(cat "$RF/state/slot/toolchains-local.txt")" = "node v22.99.0" ] &&
  grep -q 'no manifest from node-1: toolchains-node.txt kept (rsync: connection unexpectedly closed)' "$W/err" &&
  [ -z "$(rf_left)" ] &&
  ok "refresh: a node that does not answer keeps its last manifest, exit 1, the reason on stderr, no temporary file" ||
  bad "refresh no node: rc=$rc $(cat "$W/err") $(ls "$RF/state/slot")"
# An empty file from the node is no manifest.
: >"$RF/node-manifest"
rf node-1 0
rc=$?
[ $rc -eq 1 ] && [ "$(cat "$RF/state/slot/toolchains-node.txt")" = "node v22.23.3" ] &&
  [ -z "$(rf_left)" ] &&
  ok "refresh: an empty file from the node keeps the last manifest" ||
  bad "refresh empty: rc=$rc $(cat "$RF/state/slot/toolchains-node.txt")"
# toolchains.sh fails here: the local manifest stays, the node's is still written.
echo "node v22.24.0" >"$RF/node-manifest"
rm "$RF/local-lines"
rf node-1 0
rc=$?
[ $rc -eq 1 ] && [ "$(cat "$RF/state/slot/toolchains-local.txt")" = "node v22.99.0" ] &&
  [ "$(cat "$RF/state/slot/toolchains-node.txt")" = "node v22.24.0" ] &&
  grep -q 'toolchains-local.txt kept' "$W/err" &&
  ok "refresh: a failed toolchains.sh keeps the local manifest and still reads the node's" ||
  bad "refresh local fails: rc=$rc $(cat "$W/err")"
# toolchains.sh exits 0 and prints nothing: that is no manifest either.
: >"$RF/local-lines"
rf node-1 0
rc=$?
[ $rc -eq 1 ] && [ "$(cat "$RF/state/slot/toolchains-local.txt")" = "node v22.99.0" ] &&
  grep -q 'toolchains-local.txt kept' "$W/err" && [ -z "$(rf_left)" ] &&
  ok "refresh: a toolchains.sh that prints nothing keeps the local manifest" ||
  bad "refresh local empty: rc=$rc local=$(cat "$RF/state/slot/toolchains-local.txt")"
# TOOLCHAINS_NODE_FILE names the node's file.
echo "node v22.23.3" >"$RF/local-lines"
TOOLCHAINS_NODE_FILE=/var/lib/builder/manifest.txt rf node-1 0
grep -q ' node-1:/var/lib/builder/manifest.txt ' "$RF/rsync.args" &&
  ok "refresh: TOOLCHAINS_NODE_FILE names the node's file" || bad "refresh file: $(cat "$RF/rsync.args")"
# A node name that could be an option or carry a command, and a SECONDS that is no number.
rm -f "$RF/rsync.args"
st=
for n in "" "-v" "-oProxyCommand=x" "node 1" 'node;id' 'a/b'; do
  rf "$n" 0
  [ $? -eq 2 ] || st="$st [$n]"
done
rf node-1 soon
[ $? -eq 2 ] || st="$st [soon]"
RFC="$RF/it's"
rf node-1 0
[ $? -eq 2 ] || st="$st [quote in the config path]"
RFC=
[ -z "$st" ] && [ ! -e "$RF/rsync.args" ] &&
  ok "refresh: a node name that is an option or holds a separator, a SECONDS that is no number and a quote in the config path are refused" ||
  bad "refresh usage accepted:$st"
# The loop: a failed run does not end it, and it keeps the two files for its node.
rm -f "$RF/node-manifest" "$RF/state/slot/toolchains-node.txt"
HOME=$RF/home XDG_STATE_HOME=$RF/state XDG_CONFIG_HOME=${RFC:-$RF/config} PATH=$RF/bin:$PATH \
  sh "$RF/lib/toolchains-refresh.sh" node-1 1 >"$RF/loop.out" 2>"$RF/loop.err" &
rfpid=$!
i=0
until grep -q 'no manifest from node-1' "$RF/loop.err" 2>/dev/null || [ $i -ge 10 ]; do
  sleep 1
  i=$((i + 1))
done
echo "node v22.30.0" >"$RF/node-manifest"
i=0
until [ -s "$RF/state/slot/toolchains-node.txt" ] || [ $i -ge 10 ]; do
  sleep 1
  i=$((i + 1))
done
grep -q 'no manifest from node-1' "$RF/loop.err" && kill -0 $rfpid 2>/dev/null &&
  [ "$(cat "$RF/state/slot/toolchains-node.txt" 2>/dev/null)" = "node v22.30.0" ] &&
  ok "refresh: the loop goes on after a run that failed and writes the manifest on a later one" ||
  bad "refresh loop: $(cat "$RF/loop.err") node=$(cat "$RF/state/slot/toolchains-node.txt" 2>&1)"
echo "node v99.0.0" >"$RF/node-manifest"
rf node-2 0
rc=$?
[ $rc -eq 2 ] && grep -q "another run keeps the manifests of 'node-1'" "$W/err" &&
  ok "refresh: a run for a second node is refused while a loop keeps the files of the first" ||
  bad "refresh second node: rc=$rc $(cat "$W/err")"
timeout 5 env HOME="$RF/home" XDG_STATE_HOME="$RF/state" XDG_CONFIG_HOME="$RF/config" PATH="$RF/bin:$PATH" \
  sh "$RF/lib/toolchains-refresh.sh" node-1 5 >"$W/out" 2>"$W/err"
rc=$?
[ $rc -eq 2 ] && grep -q "another run keeps the manifests of 'node-1'" "$W/err" &&
  ok "refresh: a second loop for the same node is refused" || bad "refresh second loop: rc=$rc $(cat "$W/err")"
rf node-1 0
rc=$?
[ $rc -eq 0 ] && ok "refresh: a single run for the loop's node goes on beside it" ||
  bad "refresh single beside loop: rc=$rc $(cat "$W/err")"
kill $rfpid 2>/dev/null
wait $rfpid 2>/dev/null
rf node-2 0
rc=$?
[ $rc -eq 0 ] && [ -z "$(rf_left)" ] &&
  ok "refresh: after the loop ended the lock is free and no temporary file is left" ||
  bad "refresh after loop: rc=$rc $(cat "$W/err") $(rf_left)"
# A run that is stopped while it reads the node leaves no temporary file.
: >"$RF/rsync-block"
rm -f "$RF/rsync.dst"
HOME=$RF/home XDG_STATE_HOME=$RF/state XDG_CONFIG_HOME=$RF/config PATH=$RF/bin:$PATH \
  sh "$RF/lib/toolchains-refresh.sh" node-2 0 >"$RF/stop.out" 2>"$RF/stop.err" &
rfpid=$!
i=0
until [ -n "$(rf_left)" ] && [ -s "$RF/rsync.dst" ] || [ $i -ge 10 ]; do
  sleep 1
  i=$((i + 1))
done
held=$(rf_left)
kill $rfpid 2>/dev/null
rm -f "$RF/rsync-block"
wait $rfpid 2>/dev/null
rc=$?
[ -n "$held" ] && [ $rc -eq 143 ] && [ -z "$(rf_left)" ] &&
  ok "refresh: a run stopped while it reads the node exits 143 and leaves no temporary file" ||
  bad "refresh stopped: rc=$rc held=$held left=$(rf_left) $(cat "$RF/stop.err")"
u=$here/../slot-toolchains-refresh@.service
grep -q '^ExecStart=/bin/sh %h/.local/share/slot/toolchains-refresh.sh %i 60$' "$u" &&
  grep -q '^Restart=always$' "$u" && grep -q '^RestartSec=10$' "$u" &&
  [ "$(sed -n '/^\[Install\]$/,$p' "$u" | grep -c '^WantedBy=default.target$')" -eq 1 ] &&
  ok "refresh: the unit runs the script for its instance name, restarts it after 10 s and starts with the session" ||
  bad "refresh unit: $(cat "$u" 2>&1)"
grep -q '^RestartPreventExitStatus=2$' "$u" &&
  ok "refresh: the unit does not start the script again after a usage error or a held lock" ||
  bad "refresh unit exit 2: $(grep Restart "$u")"
grep -q '^EnvironmentFile=-%h/.config/slot/node/toolchains-refresh.env$' "$u" &&
  ok "refresh: the unit reads its PATH from an environment file of the host" ||
  bad "refresh unit environment: $(grep Environment "$u")"
# 6h. The example config carries what consumers build with: the rust toolchains and the wasm
# target, the dnf packages that exist on the node's repositories. Go and node come from the
# archive pins, so neither a golang nor a nodejs package is listed.
ex=$here/node.conf.example
sh -c '. "$1/conf.sh"; conf_read "$1/node.conf.example" BUILD_USER && echo "$RUST_TOOLCHAINS|$RUST_TARGETS|$EXTRA_PACKAGES"' sh "$here" >"$W/out" 2>"$W/err"
IFS='|' read -r ex_tc ex_tg ex_pk <"$W/out"
[ "$ex_tc" = "1.85.0 1.91.0 1.91.1 1.97 1.98.0 1.98.1 1.99.0 1.99.1 1.100.0 stable" ] && ok "example: the consumers' rust toolchains" || bad "example toolchains: '$ex_tc' $(cat "$W/err")"
[ "$ex_tg" = "1.97:x86_64-unknown-linux-musl 1.98.1:wasm32-unknown-unknown,x86_64-unknown-linux-musl 1.99.1:x86_64-unknown-linux-musl" ] && ok "example: targets per toolchain" || bad "example targets: '$ex_tg'"
for p in jq sqlite bzip2 unzip binutils rsync git gcc gcc-c++ make cmake clang clang-devel llvm20 llvm-devel alsa-lib-devel postgresql17 postgresql17-server java-21-amazon-corretto-devel java-17-amazon-corretto-devel java-11-amazon-corretto-devel java-1.8.0-amazon-corretto-devel maven xorg-x11-server-Xvfb gtk3 libnotify libXScrnSaver libXtst at-spi2-atk atk mesa-libgbm libxkbcommon nss; do
  case " $ex_pk " in *" $p "*) ;; *) bad "example EXTRA_PACKAGES lacks $p" ;; esac
done
for p in golang nodejs nodejs-npm nodejs22 mingw64-gcc musl-gcc dash mawk erlang elixir odin gh; do
  case " $ex_pk " in *" $p "*) bad "example EXTRA_PACKAGES lists $p" ;; esac
done
ok "example: package list checked"
grep -q 'not installed; a job that needs one is held by name' "$here/README.md" && ok "README names the packages that are not installed" || bad "README has no 'not installed; a job that needs one is held by name'"
sed -n '/are not installed; a job that needs one is held by name/,/^$/p' "$here/README.md" | grep -q 'musl' && bad "README still lists musl-gcc as not installed" || ok "README no longer lists musl-gcc as not installed"
sed -n '/are not installed; a job that needs one is held by name/,/^$/p' "$here/README.md" | grep -q 'dash' && bad "README still lists dash as not installed" || ok "README no longer lists dash as not installed"
grep -qx 'GO_ROOT=' "$ex" && [ "$(sh -c '. "$1/conf.sh"; conf_read "$1/node.conf.example" BUILD_USER && echo "[$GO_ROOT]"' sh "$here" 2>&1)" = '[]' ] && ok "example: GO_ROOT is shown empty, the default" || bad "example GO_ROOT: $(grep GO_ROOT "$ex")"
grep -qx 'SLOT_TARGET=x86_64-unknown-linux-gnu' "$ex" && [ "$(sh -c '. "$1/conf.sh"; conf_read "$1/node.conf.example" BUILD_USER && echo "[$SLOT_TARGET]"; grep -v "^SLOT_TARGET=" "$1/node.conf.example" >"$2"; conf_read "$2" BUILD_USER && echo "[$SLOT_TARGET]"' sh "$here" "$W/ex-no-target" 2>&1 | tr '\n' ' ')" = '[x86_64-unknown-linux-gnu] [x86_64-unknown-linux-gnu] ' ] && ok "example: SLOT_TARGET is shown with its default, the archive a node installs without the key" || bad "example SLOT_TARGET: $(grep SLOT_TARGET "$ex")"

# 6y. The gate's step for the shell tests (contrib/shell-tests.sh): under dash where dash is on
# PATH, else under sh; its first line says which; every test runs once; the first failure stops it.
# `dash` here is a stand-in that writes its arguments down and runs the machine's sh.
ST=$W/st
SS=$here/../shell-tests.sh
mkdir -p "$ST/bin" "$ST/dashbin"
for t in sh mktemp ln rm readlink; do ln -s "$(command -v "$t")" "$ST/bin/$t"; done
printf '#!/bin/sh\necho "$*" >>"%s/dash.log"\nexec /bin/sh "$@"\n' "$ST" >"$ST/dashbin/dash"
chmod 755 "$ST/dashbin/dash"
for t in t1 t2 t4; do printf 'echo "%s $(command -v sh)" >>"%s/ran.log"\nsh -c "echo nested-%s" >/dev/null\n' "$t" "$ST" "$t" >"$ST/$t.sh"; done
printf 'exit 1\n' >"$ST/t3.sh"
: >"$ST/dash.log"
: >"$ST/ran.log"
PATH=$ST/dashbin:$ST/bin sh "$SS" "$ST/t1.sh" "$ST/t2.sh" >"$W/out" 2>"$W/err"
st=$?
[ $st -eq 0 ] && [ "$(head -n 1 "$W/out")" = "shell tests: under dash ($ST/dashbin/dash), which is sh for the tests" ] && [ "$(grep -c '^shell tests: ' "$W/out")" -eq 1 ] && ok "shell tests: with dash on PATH one line names dash" || bad "shell tests under dash: exit $st $(cat "$W/out" "$W/err")"
[ "$(cut -d' ' -f1 "$ST/ran.log" | tr '\n' ' ')" = "t1 t2 " ] && ok "shell tests: each test ran once, in order" || bad "shell tests ran: $(cat "$ST/ran.log")"
[ "$(grep -c "^$ST/t[12]\.sh\$" "$ST/dash.log")" -eq 2 ] && [ "$(grep -c '^-c echo nested-t[12]$' "$ST/dash.log")" -eq 2 ] && ok "shell tests: dash runs each test, and the sh a test starts by name" || bad "shell tests dash calls: $(tr '\n' '|' <"$ST/dash.log")"
sd=$(sed -n '1s/^t1 //p' "$ST/ran.log")
[ -n "$sd" ] && [ "$sd" != "$ST/bin/sh" ] && [ ! -e "$(dirname "$sd")" ] && ok "shell tests: the directory of the sh link is gone at the end" || bad "shell tests: sh was '$sd', directory left: $(ls -d "$(dirname "$sd")" 2>&1)"
: >"$ST/dash.log"
: >"$ST/ran.log"
PATH=$ST/bin sh "$SS" "$ST/t1.sh" "$ST/t2.sh" >"$W/out" 2>"$W/err"
st=$?
[ $st -eq 0 ] && [ "$(cat "$W/out")" = "shell tests: under sh ($(readlink -f "$ST/bin/sh")): no dash on PATH" ] && ok "shell tests: without dash on PATH one line names the machine's sh" || bad "shell tests under sh: exit $st $(cat "$W/out" "$W/err")"
[ "$(tr '\n' ',' <"$ST/ran.log")" = "t1 $ST/bin/sh,t2 $ST/bin/sh," ] && [ ! -s "$ST/dash.log" ] && ok "shell tests: without dash each test ran once under sh" || bad "shell tests under sh ran: $(cat "$ST/ran.log" "$ST/dash.log")"
: >"$ST/ran.log"
PATH=$ST/dashbin:$ST/bin sh "$SS" "$ST/t1.sh" "$ST/t3.sh" "$ST/t4.sh" >"$W/out" 2>"$W/err"
st=$?
[ $st -eq 1 ] && [ "$(cut -d' ' -f1 "$ST/ran.log" | tr '\n' ' ')" = "t1 " ] && ok "shell tests: a failing test ends the step with status 1, and the tests after it do not run" || bad "shell tests failure: exit $st $(cat "$ST/ran.log")"
PATH=$ST/bin sh "$SS" >"$W/out" 2>"$W/err"
[ $? -eq 2 ] && ok "shell tests: no test named is a usage error" || bad "shell tests without arguments: $(cat "$W/out" "$W/err")"
tf=$here/../../Taskfile.yml
grep -qx '      - sh contrib/shell-tests.sh contrib/slot-shim.test.sh contrib/rustc-gate.test.sh contrib/node/test.sh' "$tf" && [ "$(grep -c 'contrib/node/test\.sh' "$tf")" -eq 1 ] && [ "$(grep -c 'contrib/slot-shim\.test\.sh' "$tf")" -eq 1 ] && [ "$(grep -c 'contrib/rustc-gate\.test\.sh' "$tf")" -eq 1 ] && ok "the gate runs the three shell tests through shell-tests.sh, each once" || bad "Taskfile: $(grep -n 'contrib/' "$tf" | tr '\n' ' ')"

# 6z. Firefox on a node: an archive the node installs only when its config names it
# (EXTRA_ARCHIVES, the lines of EXTRA_ARCHIVE_PINS), with the packages it loads. A consumer's
# browser tests start `firefox` by name, headless. The release archive of Mozilla is unpacked into
# /usr/local/firefox-<release> and linked as /usr/local/bin/firefox; `check --canary` starts it
# headless as the build user until it listens for WebDriver BiDi.
FZ=/usr/local/firefox-159.0
mkdir -p "$W/r0"
fzpin=$(sed -n "s/^EXTRA_ARCHIVE_PINS='\(firefox .*\)'$/\1/p" "$P")
[ "$fzpin" = 'firefox 159.0 54536fcd120af484d4c56272a5a1cf4af57b9c713d1a3b1e775da192dd541fcd https://archive.mozilla.org/pub/firefox/releases/159.0/linux-x86_64/en-US/firefox-159.0.tar.xz 1 firefox' ] && ok "the firefox pin: release 159.0, the SHA-256 of Mozilla's SHA256SUMS, one leading directory stripped, the binary firefox" || bad "firefox pin: '$fzpin'"
for fz_bad in 'EXTRA_ARCHIVES=fire;fox' 'EXTRA_ARCHIVES=$(id)' 'EXTRA_ARCHIVES=firefox -x' 'EXTRA_ARCHIVES=a*'; do
  conf
  echo "$fz_bad" >>"$W/node.conf"
  run "$W/r0" check
  [ $? -eq 2 ] && grep -q "EXTRA_ARCHIVES must be names" "$W/err" && ok "'$fz_bad' is refused by the config's form" || bad "'$fz_bad' is accepted ($(cat "$W/err"))"
done
conf
echo 'EXTRA_ARCHIVES=firefox chrome' >>"$W/node.conf"
run "$W/r0" check
[ $? -eq 2 ] && grep -qx "node config: $W/node.conf: EXTRA_ARCHIVES names 'chrome', which is no archive of provision.sh (firefox)" "$W/err" && ok "EXTRA_ARCHIVES with a name provision.sh has no archive for is refused, and the names it has are listed" || bad "unknown extra archive: $(cat "$W/out" "$W/err" | tail -3)"
sh "$P" bundle "$W/node.conf" check >/dev/null 2>"$W/err"
[ $? -eq 2 ] && grep -q "EXTRA_ARCHIVES names 'chrome'" "$W/err" && ok "a bundle of such a config is refused too" || bad "bundle with an unknown extra archive: $(cat "$W/err")"
FXR=$W/rfx
mkdir -p "$FXR"
echo 8 >"$FXR/.state-nproc"
conf
run "$FXR" apply
run "$FXR" check
[ $? -eq 0 ] && [ ! -e "$FXR$FZ" ] && [ ! -L "$FXR/usr/local/bin/firefox" ] && ! grep -q firefox "$W/out" && ok "without EXTRA_ARCHIVES a node has no firefox and check names none" || bad "firefox without the key: $(grep -i firefox "$W/out") $(ls "$FXR/usr/local" 2>&1 | tr '\n' ' ')"
grep -qx 'gtk3' "$FXR/.state/packages" && bad "without EXTRA_ARCHIVES the packages of firefox are installed" || ok "without EXTRA_ARCHIVES the packages of firefox are not installed"
echo 'EXTRA_ARCHIVES=firefox' >>"$W/node.conf"
run "$FXR" check
st=$?
[ $st -eq 1 ] && grep -qx "drift: archive:firefox: /usr/local/bin/firefox is 'missing', want a link to $FZ/firefox" "$W/out" && grep -qx 'drift: packages: not installed: gtk3 alsa-lib libX11-xcb dbus-libs xz' "$W/out" && ok "EXTRA_ARCHIVES=firefox: check names the link and the packages firefox loads" || bad "firefox drift: exit $st $(grep -i 'firefox\|packages' "$W/out")"
: >"$FXR/.calls"
run "$FXR" apply
st=$?
[ $st -eq 0 ] && grep -qx 'fetch https://archive.mozilla.org/pub/firefox/releases/159.0/linux-x86_64/en-US/firefox-159.0.tar.xz' "$FXR/.calls" && [ "$(readlink "$FXR/usr/local/bin/firefox")" = "$FZ/firefox" ] && ok "apply fetches the archive of Firefox 159.0 and links /usr/local/bin/firefox to $FZ/firefox" || bad "firefox apply: exit $st $(grep firefox "$FXR/.calls") '$(readlink "$FXR/usr/local/bin/firefox")' $(grep firefox "$W/err")"
for p in gtk3 alsa-lib libX11-xcb dbus-libs xz; do grep -qx "$p" "$FXR/.state/packages" || bad "apply did not install $p"; done
[ "$("$FXR$FZ/firefox" --version 2>&1)" = 'Mozilla Firefox 159.0' ] && ok "the stand-in answers as Firefox does: Mozilla Firefox 159.0" || bad "firefox stand-in: $("$FXR$FZ/firefox" --version 2>&1)"
run "$FXR" check
[ $? -eq 0 ] && ok "after apply: firefox and its packages, 0 drift" || bad "firefox after apply: $(cat "$W/out")"
sh "$here/toolchains.sh" "$FXR" "$W/norust" 2>/dev/null | grep '^tool firefox' >"$W/out2"
[ "$(cat "$W/out2")" = "tool firefox 159.0" ] && ok "toolchains.sh: one line, tool firefox 159.0" || bad "tool firefox line: '$(cat "$W/out2")'"
mkdir -p "$W/fzp/usr/bin"
printf '#!/bin/sh\n[ "$1" = --version ] && [ "${MOZ_HEADLESS:-}" = 1 ] && echo "Mozilla Firefox 158.0.1"\n' >"$W/fzp/usr/bin/firefox"
chmod 755 "$W/fzp/usr/bin/firefox"
[ "$(sh "$here/toolchains.sh" "$W/fzp" "$W/norust" 2>/dev/null | grep '^tool firefox')" = "tool firefox 158.0.1" ] && ok "toolchains.sh: a workstation's firefox in /usr/bin, read headless" || bad "tool firefox of /usr/bin: '$(sh "$here/toolchains.sh" "$W/fzp" "$W/norust" 2>/dev/null | grep '^tool firefox')'"
printf '#!/bin/sh\nexit 1\n' >"$W/fzp/usr/bin/firefox"
[ "$(sh "$here/toolchains.sh" "$W/fzp" "$W/norust" 2>/dev/null | grep '^tool firefox')" = "tool firefox missing" ] && ok "toolchains.sh: a firefox that prints nothing is missing" || bad "tool firefox silent: '$(sh "$here/toolchains.sh" "$W/fzp" "$W/norust" 2>/dev/null | grep '^tool firefox')'"
cp "$FXR$FZ/firefox" "$W/firefox.keep"
printf '#!/bin/sh\necho "Mozilla Firefox 158.0"\n' >"$FXR$FZ/firefox"
run "$FXR" check
grep -qx "drift: archive:firefox: $FZ/firefox says 'Mozilla Firefox 158.0', want 159.0" "$W/out" && ok "check names a firefox of another release" || bad "firefox version drift: $(grep firefox "$W/out")"
run "$FXR" apply
run "$FXR" check
[ $? -eq 0 ] && cmp -s "$W/firefox.keep" "$FXR$FZ/firefox" && ok "apply installs the pinned release again" || bad "firefox restore: $(cat "$W/out")"
# The real branch of archive_fetch with an archive of the shape of Mozilla's: a .tar.xz with one
# top directory, firefox/, which is stripped. curl is a stand-in that serves it.
if command -v xz >/dev/null 2>&1; then
  mkdir -p "$W/fzsrc/firefox" "$W/fzfetch"
  printf '#!/bin/sh\necho "Mozilla Firefox 159.0"\n' >"$W/fzsrc/firefox/firefox"
  chmod 755 "$W/fzsrc/firefox/firefox"
  tar -cJf "$W/fzsrc.tar.xz" -C "$W/fzsrc" firefox
  fztree=$(
    eval "$(sed -n '/^archive_src() /p; /^archive_fetch() {$/,/^}$/p' "$P")"
    fake_fetch() { false; }
    curl() { cp "$W/fzsrc.tar.xz" "$2"; }
    # shellcheck disable=SC2086
    set -- $fzpin
    fzsum=$(sha256sum "$W/fzsrc.tar.xz" | cut -d' ' -f1)
    archive_fetch "$W/fzfetch" "$1" "$2" "$fzsum" "$4" "$5" "$6" 2>&1
  )
  [ "$fztree" = "$W/fzfetch/tree" ] && [ "$("$W/fzfetch/tree/firefox" --version 2>&1)" = 'Mozilla Firefox 159.0' ] && ok "the firefox pin through the real unpack: a .tar.xz, firefox/ is stripped" || bad "firefox unpack: '$fztree' $(find "$W/fzfetch" 2>&1 | head -5)"
else
  ok "the firefox pin through the real unpack: not run, no xz on this machine"
fi
# The canary: check --canary starts firefox as the build user in a new directory of the store's
# tmp (its HOME, TMPDIR and profile), headless with a WebDriver BiDi port, waits for the line
# `WebDriver BiDi listening on ws://...` on stderr, ends it and removes the directory. The
# stand-in of apply prints that line and waits; the failures are stand-ins of this test.
fzc() { NODE_TEST_FIREFOX_WAIT=2 NODE_TEST=1 NODE_ROOT=$FXR sh "$P" check "$W/node.conf" "$@" >"$W/out" 2>"$W/err"; }
fzc
! grep -q 'firefox' "$W/out" && ok "check without --canary starts no firefox" || bad "check alone: $(grep firefox "$W/out")"
fzc --canary
grep -qx 'canary: ok: firefox: firefox 159.0 started headless as the build user and listened for WebDriver BiDi (ws://127.0.0.1:9222)' "$W/out" && ! grep -q '^drift: firefox' "$W/out" && ok "check --canary: firefox starts headless and listens for WebDriver BiDi, one ok line" || bad "firefox canary: $(grep firefox "$W/out" "$W/err")"
[ -z "$(ls -A "$FXR/srv/slot/tmp" 2>/dev/null | grep firefox)" ] && ok "the firefox canary leaves no directory in the store's tmp" || bad "firefox canary left: $(ls -A "$FXR/srv/slot/tmp")"
cat >"$FXR$FZ/firefox" <<EOF
#!/bin/sh
[ "\$1" != --version ] || { echo "Mozilla Firefox 159.0"; exit 0; }
echo "\$*" >"$W/fz-args"
echo "\$HOME|\$TMPDIR|\${MOZ_HEADLESS:-}" >"$W/fz-env"
pwd >"$W/fz-pwd"
echo "*** You are running in headless mode." >&2
echo "XPCOMGlueLoad error for file $FZ/libxul.so:" >&2
exit 1
EOF
fzc --canary
st=$?
[ $st -eq 1 ] && grep -qx 'drift: firefox: canary: firefox exited with status 1 before it listened for WebDriver BiDi: XPCOMGlueLoad error for file /usr/local/firefox-159.0/libxul.so:' "$W/out" && ! grep -q '^canary: ok: firefox' "$W/out" && ok "check --canary: a firefox that exits before the BiDi line is drift with its first line that is no headless notice" || bad "firefox canary exit: exit $st $(grep firefox "$W/out" "$W/err")"
case $(cat "$W/fz-args" 2>&1) in
  "--headless --no-remote --remote-debugging-port 0 --profile $FXR/srv/slot/tmp/slot-firefox-canary."*/profile' about:blank') ok "the canary starts firefox headless, without remote, with a BiDi port and its own profile" ;;
  *) bad "firefox canary arguments: $(cat "$W/fz-args" 2>&1)" ;;
esac
fzd=$(cat "$W/fz-pwd" 2>&1)
case $fzd in
  "$FXR"/srv/slot/tmp/slot-firefox-canary.*) [ ! -e "$fzd" ] && [ "$(cat "$W/fz-env")" = "$fzd/home|$fzd/tmp|1" ] && ok "the canary runs in a directory of the store's tmp with its HOME and TMPDIR there, and removes it" || bad "firefox canary directory: $fzd $(cat "$W/fz-env")" ;;
  *) bad "firefox canary directory: $fzd" ;;
esac
printf '#!/bin/sh\n[ "$1" != --version ] || { echo "Mozilla Firefox 159.0"; exit 0; }\necho "*** You are running in headless mode." >&2\nexec sleep 30\n' >"$FXR$FZ/firefox"
fzc --canary
st=$?
[ $st -eq 1 ] && grep -qxF 'drift: firefox: canary: no WebDriver BiDi line from firefox within 2 s: *** You are running in headless mode.' "$W/out" && ok "check --canary: a firefox that does not listen within the wait is drift" || bad "firefox canary wait: exit $st $(grep firefox "$W/out" "$W/err")"
cp "$W/firefox.keep" "$FXR$FZ/firefox"
conf
fzc --canary
! grep -q 'firefox' "$W/out" && ok "check --canary without EXTRA_ARCHIVES=firefox starts no firefox" || bad "canary without the key: $(grep firefox "$W/out")"
grep -qx 'EXTRA_ARCHIVES=firefox' "$here/node.conf.example" && ok "example: EXTRA_ARCHIVES=firefox" || bad "example EXTRA_ARCHIVES: $(grep EXTRA_ARCHIVES "$here/node.conf.example")"
grep -q 'EXTRA_ARCHIVES' "$here/README.md" && grep -q 'firefox' "$here/README.md" && ok "README names EXTRA_ARCHIVES and firefox" || bad "README lacks EXTRA_ARCHIVES or firefox"

# 7. Nothing private in the repository's node files: no home of a real user, no cloud ids, no
# addresses but loopback and the metadata service.
leaks=$(grep -nE '/home/[a-z]|\b(i|vol|subnet|sg|vpc|ami)-0?[0-9a-f]{8,}\b|[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[a-z]{2,}|\b[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\b' \
  "$here"/*.sh "$here"/node.conf.example "$here"/README.md 2>/dev/null |
  grep -vE '169\.254\.169\.254|127\.0\.0\.1|0\.0\.0\.0' | grep -v 'test.sh:')
[ -z "$leaks" ] && ok "no private values in contrib/node" || bad "private values: $leaks"

echo "$fails failed"
[ $fails -eq 0 ]
