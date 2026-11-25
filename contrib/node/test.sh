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
for bad_rust in 'RUST_TOOLCHAINS=stable;id' 'RUST_COMPONENTS=clippy*' 'RUST_TARGETS=$(id)' 'RUST_TOOLCHAINS=1.99.1 -stable' 'EXTRA_PACKAGES=jq;id' 'EXTRA_PACKAGES=git --nogpgcheck' 'EXTRA_PACKAGES=a $(id)' 'EXTRA_PACKAGES=a*'; do
  conf
  echo "$bad_rust" >>"$W/node.conf"
  run "$W/r0" check
  if [ $? -eq 2 ] && grep -q "${bad_rust%%=*} must be names" "$W/err"; then ok "${bad_rust%%=*} with a shell character is refused"; else bad "${bad_rust%%=*} with a shell character is accepted ($(cat "$W/err"))"; fi
done
conf
sed -i 's/^BUILD_UID=.*/BUILD_UID=10x1/' "$W/node.conf"
run "$W/r0" check
if [ $? -eq 2 ] && grep -q 'BUILD_UID must be a whole number' "$W/err"; then ok "malformed BUILD_UID is refused"; else bad "malformed BUILD_UID is refused ($(cat "$W/err"))"; fi

# 2. A fresh machine drifts in every part; apply brings it to 0 drift.
conf
R=$W/r1
mkdir -p "$R"
echo 64 >"$R/.state-nproc"
run "$R" check
st=$?
n=$(grep -c '^drift: ' "$W/out")
if [ $st -eq 1 ] && [ "$n" -ge 15 ]; then ok "fresh machine: check exits 1 with $n drift lines"; else bad "fresh machine: check exit $st, $n drift lines"; fi
for item in storage binds packages user unit:slot-node.service unit:slot-storage.service keys sshd sudoers slot:/usr/local/bin/slot rustup rust:1.99.1 rust:stable; do
  grep -q "^drift: $item" "$W/out" || bad "fresh machine: no drift line for $item"
done
: >"$R/.calls"
run "$R" apply
cp "$W/out" "$W/first-apply.out"
st=$?
[ $st -eq 0 ] && ok "apply exits 0" || bad "apply exit $st: $(tail -3 "$W/err")"
# A naked node needs nothing copied: one apply starts both services and leaves 0 drift.
grep -q '^pending: ' "$W/err" && bad "apply on a naked machine leaves a pending step: $(grep '^pending: ' "$W/err")" || ok "apply on a naked machine leaves nothing pending"
for u in slot-node.service sccache-node.service; do
  [ -f "$R/.state/active/$u" ] && ok "one apply starts $u" || bad "$u not started by the first apply"
done
run "$R" check
st=$?
if [ $st -eq 0 ] && grep -q '^0 drift$' "$W/out"; then ok "after one apply: 0 drift"; else bad "after one apply: exit $st, $(cat "$W/out")"; fi
# The units use the node's own /usr and /etc.
for u in slot-node.service sccache-node.service; do
  grep -qE 'rootfs|BindReadOnlyPaths|ConditionPathExists|workstation' "$R/etc/systemd/system/$u" && bad "$u still binds or waits for a copy: $(grep -E 'rootfs|BindReadOnlyPaths|ConditionPathExists|workstation' "$R/etc/systemd/system/$u" | head -2)" || ok "$u uses the node's own /usr and /etc"
done
for u in slot-node.service sccache-node.service; do
  grep -q '^Environment=PATH=/srv/slot/nodebin:/var/lib/builder/.cargo/bin:/usr/local/bin:/usr/bin:' "$R/etc/systemd/system/$u" && ok "$u PATH: nodebin, cargo, /usr/local/bin, /usr/bin" || bad "$u PATH: $(grep PATH "$R/etc/systemd/system/$u")"
  grep -q '/usr/local/go/bin' "$R/etc/systemd/system/$u" && bad "$u PATH still has /usr/local/go/bin" || ok "$u PATH: no /usr/local/go/bin (the go link is in /usr/local/bin)"
done
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
run "$R" apply
grep -q -- '--tokens 32 ' "$R/etc/systemd/system/slot-node.service" && ok "32 CPUs: --tokens 32" || bad "32 CPUs: $(grep ExecStart "$R/etc/systemd/system/slot-node.service")"

# 4. Each kind of change on the machine is found, named, and closed by apply.
echo 'Environment=EXTRA=1' >>"$R/etc/systemd/system/sccache-node.service"
echo "/srv/slot/home/.cache /var/lib/builder/.cache none bind 0 0" >>"$R/etc/fstab"
k=$R/var/lib/builder/.ssh/authorized_keys
rm -f "$R/.state/immutable/$(echo "$k" | tr / _)"
echo 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAnotherKey other' >>"$k"
printf '#!/bin/sh\necho slot 0.35.0\n' >"$R/srv/slot/bin/slot"
rm -f "$R/var/lib/builder/.cargo"
grep -v '^MaxSessions' "$R/etc/ssh/sshd_config" >"$W/s" && cat "$W/s" >"$R/etc/ssh/sshd_config"
sed -i '\# /var/lib/builder/.cache$#d' "$R/.state/mounts"
run "$R" check
st=$?
for item in unit:sccache-node.service fstab keys slot:/srv/slot/bin/slot link:/var/lib/builder/.cargo sshd binds; do
  grep -q "^drift: $item" "$W/out" && ok "found: $item" || bad "not found: $item ($(cat "$W/out"))"
done
[ $st -eq 1 ] || bad "check with drift exits $st"
run "$R" apply
run "$R" check
st=$?
if [ $st -eq 0 ]; then ok "apply closes every drift"; else bad "left after apply: $(cat "$W/out")"; fi
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

# 5e. Cargo tools pinned in provision.sh (the release a consumer's CI installs) are installed at
# that release in the store's cargo bin and read back; check names one of another release.
sh "$P" pins >"$W/pins" 2>"$W/err"
grep -qx 'wasm-bindgen 0.2.130' "$W/pins" && grep -qx 'wasm-bindgen-test-runner 0.2.130' "$W/pins" &&
  ok "pins: wasm-bindgen 0.2.130" || bad "pins: $(cat "$W/pins" "$W/err")"
# geckodriver at the release of the forge runner image of the browser test lanes.
grep -qx 'geckodriver 0.37.1' "$W/pins" && ok "pins: geckodriver 0.37.1" || bad "pins: no geckodriver 0.37.1: $(cat "$W/pins")"
# sccache: the node has no /usr from the workstation, and the distribution has no package of it.
grep -qx 'sccache 0.18.0' "$W/pins" && ok "pins: sccache 0.18.0" || bad "pins: no sccache 0.18.0: $(cat "$W/pins")"
# ess is not pinned: it picks the release a repository pins itself (an exact `requires` runs
# that release from its own digest-checked cache, a line pin runs the base binary).
grep -q '^ess ' "$W/pins" && bad "ess is pinned on the node: $(grep '^ess ' "$W/pins")" || ok "ess is not pinned"
while read -r b v; do
  [ "$("$R/srv/slot/cargo/bin/$b" --version 2>/dev/null)" = "$b $v" ] && ok "pinned tool installed: $b $v" || bad "pinned tool $b: '$("$R/srv/slot/cargo/bin/$b" --version 2>&1)'"
done <"$W/pins"
printf '#!/bin/sh\necho wasm-bindgen 0.2.129\n' >"$R/srv/slot/cargo/bin/wasm-bindgen"
run "$R" check
grep -q "^drift: tool:wasm-bindgen: .*'wasm-bindgen 0.2.129', want wasm-bindgen 0.2.130" "$W/out" && ok "check names a wasm-bindgen of another release" || bad "wasm-bindgen 0.2.129 not named: $(grep tool "$W/out")"
grep -q '^drift: tool:ess' "$W/out" && bad "check names ess" || ok "check leaves ess to push.sh"
: >"$R/.state-tool-fetch-fails"
run "$R" apply
st=$?
rm -f "$R/.state-tool-fetch-fails"
[ $st -eq 1 ] && grep -q '^failed: tool:wasm-bindgen: download or checksum of wasm-bindgen 0.2.130' "$W/err" && ok "a failed tool download: apply exits 1 and names it" || bad "failed tool download: exit $st $(cat "$W/err")"
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
rm -f "$R/srv/slot/cargo/bin/cargo-nextest" "$R/srv/slot/cargo/bin/task"
: >"$R/.calls"
run "$R" apply
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
mkdir -p "$TC/usr/bin" "$TC/usr/lib/go/bin" "$TC/rh/toolchains/1.99.1-x86_64-unknown-linux-gnu/bin" "$TC/rh/toolchains/1.99.1-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu"
printf '#!/bin/sh\necho "commit-hash: abc123"\n' >"$TC/rh/toolchains/1.99.1-x86_64-unknown-linux-gnu/bin/rustc"
printf '#!/bin/sh\necho "go version go1.24.0 linux/amd64"\n' >"$TC/usr/lib/go/bin/go"
printf '#!/bin/sh\necho v22.1.0\n' >"$TC/usr/bin/node"
printf '#!/bin/sh\necho "gcc (stub) 13.2.0"\necho second line\n' >"$TC/usr/bin/gcc"
chmod 755 "$TC/rh/toolchains/1.99.1-x86_64-unknown-linux-gnu/bin/rustc" "$TC/usr/lib/go/bin/go" "$TC/usr/bin/node" "$TC/usr/bin/gcc"
sh "$here/toolchains.sh" "$TC" "$TC/rh" >"$W/out" 2>"$W/err"
grep -q '^rust 1.99.1-x86_64-unknown-linux-gnu abc123 [0-9a-f]\{12\}$' "$W/out" && ok "toolchains.sh: a rust line with name, commit and digest" || bad "rust line: $(cat "$W/out" "$W/err")"
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
grep -qx 'node v22.1.0' "$W/out" && ok "toolchains.sh: node prints its version" || bad "node line: $(grep '^node' "$W/out")"
grep -qx 'c gcc gcc (stub) 13.2.0' "$W/out" && ok "toolchains.sh: gcc prints the first line of its version" || bad "gcc line: $(grep '^c gcc' "$W/out")"
grep -qx 'c clang missing' "$W/out" && ok "toolchains.sh: a missing compiler is named" || bad "clang line: $(grep '^c clang' "$W/out")"
grep -q '^c usr \|^c etc \|^c mingw-sysroot' "$W/out" && bad "toolchains.sh still digests /usr, /etc or the mingw sysroot: $(grep '^c usr\|^c etc\|^c mingw' "$W/out")" || ok "toolchains.sh: no digest of /usr, /etc or the mingw sysroot"

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
# whole task; a step that needs docker or a local service goes in `.slot.toml` `local`.
for b in no-credentials ssh; do
  msg=$(sh "$R/srv/slot/nodebin/$b" host true 2>&1 >/dev/null)
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
for part in slot tool:wasm-bindgen tool:geckodriver tool:sccache tool:cargo-nextest tool:task rustup archive:go archive:node; do
  grep -q "^failed: $part: " "$W/err" && ok "wrong SHA-256: $part fails" || bad "wrong SHA-256: no failed line for $part: $(grep '^failed' "$W/err" | head -3)"
done
[ ! -s "$W/tar.log" ] && ok "wrong SHA-256: tar never runs" || bad "tar ran on an unchecked file: $(cat "$W/tar.log")"
[ ! -e "$W/ran" ] && ok "wrong SHA-256: no downloaded file runs" || bad "a downloaded file ran"
[ "$(grep -c 'rustup-init\|go1.27.1\|node-v22\|task_linux\|nextest\|geckodriver\|wasm-bindgen\|sccache\|slot-0.36' "$W/curl.log")" -ge 9 ] && ok "wrong SHA-256: every download was attempted" || bad "curl calls: $(cat "$W/curl.log")"
# Control: with the SHA256SUMS line that matches the served file, tar is reached (so the stand-in
# tar would record a call if the check had failed open above), with no owner taken from the archive.
RG=$W/r6b
mkdir -p "$RG"
printf '#!/bin/sh\ntouch "%s/ran"\n' "$W" | sha256sum | cut -d' ' -f1 >"$W/slot-sum"
: >"$W/tar.log"
realrun "$RG"
grep -q 'xzf slot-0.36.0-x86_64-unknown-linux-gnu.tar.gz' "$W/tar.log" && ok "control: a matching SHA-256 reaches tar" || bad "control: tar not reached: $(cat "$W/tar.log")"
grep -q -- '--no-same-owner' "$W/tar.log" && ok "slot's tar takes no owner from the archive" || bad "slot's tar has no --no-same-owner: $(cat "$W/tar.log")"
[ "$(grep -c 'no-same-owner' "$here/provision.sh")" -ge 3 ] && ok "every tar of provision.sh that runs as root has --no-same-owner" || bad "--no-same-owner count: $(grep -c 'no-same-owner' "$here/provision.sh")"
[ "$(grep -c 'u-s,g-s' "$here/provision.sh")" -ge 3 ] && ok "unpacked trees lose setuid and setgid (static check)" || bad "u-s,g-s count: $(grep -c 'u-s,g-s' "$here/provision.sh")"

# 6j. After apply changed a unit file or replaced the slot binary, each running unit it affects
# restarts: sccache-node always, slot-node when no job runs (else a pending line). check reports a
# running unit whose loaded definition, process environment or slot binary is not what its file
# and the config say.
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
grep -qx 'systemctl restart sccache-node.service' "$R7/.calls" && ok "a changed unit file: sccache-node restarts" || bad "no sccache-node restart: $(cat "$R7/.calls")"
grep -qx 'systemctl restart slot-node.service' "$R7/.calls" && bad "slot-node restarted with a job running" || ok "a changed unit file: slot-node waits while a job runs"
grep -q '^pending: running: slot-node' "$W/err" && ok "slot-node's restart is a pending line" || bad "no pending line: $(cat "$W/err")"
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

# 6h. The example config carries what consumers build with: the rust toolchains and the wasm
# target, the dnf packages that exist on the node's repositories. Go and node come from the
# archive pins, so neither a golang nor a nodejs package is listed.
ex=$here/node.conf.example
sh -c '. "$1/conf.sh"; conf_read "$1/node.conf.example" BUILD_USER && echo "$RUST_TOOLCHAINS|$RUST_TARGETS|$EXTRA_PACKAGES"' sh "$here" >"$W/out" 2>"$W/err"
IFS='|' read -r ex_tc ex_tg ex_pk <"$W/out"
[ "$ex_tc" = "1.85.0 1.91.0 1.91.1 1.97 1.98.0 1.98.1 1.99.0 1.99.1 1.100.0 stable" ] && ok "example: the consumers' rust toolchains" || bad "example toolchains: '$ex_tc' $(cat "$W/err")"
[ "$ex_tg" = "wasm32-unknown-unknown" ] && ok "example: the wasm target" || bad "example targets: '$ex_tg'"
for p in jq sqlite bzip2 unzip binutils rsync git gcc gcc-c++ make cmake clang clang-devel llvm20 llvm-devel alsa-lib-devel postgresql17 postgresql17-server java-21-amazon-corretto-devel java-17-amazon-corretto-devel java-11-amazon-corretto-devel java-1.8.0-amazon-corretto-devel maven xorg-x11-server-Xvfb gtk3 libnotify libXScrnSaver libXtst at-spi2-atk atk mesa-libgbm libxkbcommon nss; do
  case " $ex_pk " in *" $p "*) ;; *) bad "example EXTRA_PACKAGES lacks $p" ;; esac
done
for p in golang nodejs nodejs-npm nodejs22 mingw64-gcc musl-gcc dash mawk erlang elixir odin gh; do
  case " $ex_pk " in *" $p "*) bad "example EXTRA_PACKAGES lists $p" ;; esac
done
ok "example: package list checked"
grep -q 'not installed; a job that needs one is held by name' "$here/README.md" && ok "README names the packages that are not installed" || bad "README has no 'not installed; a job that needs one is held by name'"

# 7. Nothing private in the repository's node files: no home of a real user, no cloud ids, no
# addresses but loopback and the metadata service.
leaks=$(grep -nE '/home/[a-z]|\b(i|vol|subnet|sg|vpc|ami)-0?[0-9a-f]{8,}\b|[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[a-z]{2,}|\b[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\b' \
  "$here"/*.sh "$here"/node.conf.example "$here"/README.md 2>/dev/null |
  grep -vE '169\.254\.169\.254|127\.0\.0\.1|0\.0\.0\.0' | grep -v 'test.sh:')
[ -z "$leaks" ] && ok "no private values in contrib/node" || bad "private values: $leaks"

echo "$fails failed"
[ $fails -eq 0 ]
