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
grep -q '^drift: packages: .*zstd' "$W/out" && grep -q '^drift: packages: .*python3' "$W/out" && grep -q '^drift: packages: .*glibc-static' "$W/out" && grep -q '^drift: packages: .*bubblewrap' "$W/out" && grep -q '^drift: packages: .*socat' "$W/out" && grep -q '^drift: packages: .*libubsan' "$W/out" && ok "check names zstd, python3, glibc-static, bubblewrap, socat and libubsan as packages of a bare node" || bad "base packages: $(grep '^drift: packages' "$W/out" | cut -c1-200)"
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
# bin/prettier.cjs as `prettier`, and the script is executable. pnpm is one more of them; helm is
# an archive with one binary, linked the same way.
for l in tsc:typescript-6.0.3/bin/tsc tsserver:typescript-6.0.3/bin/tsserver prettier:prettier-3.8.1/bin/prettier.cjs esbuild:esbuild-0.28.2/bin/esbuild pnpm:pnpm-11.9.0/bin/pnpm.mjs pnpx:pnpm-11.9.0/bin/pnpx.mjs helm:helm-v3.8.1/helm; do
  [ "$(readlink "$R5/usr/local/bin/${l%%:*}")" = "/usr/local/${l#*:}" ] && ok "/usr/local/bin/${l%%:*} links to /usr/local/${l#*:}" || bad "link ${l%%:*}: '$(readlink "$R5/usr/local/bin/${l%%:*}")'"
done
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
printf '#!/bin/sh\necho "GNU Make 4.4.1"\n' >"$TC/p2/usr/local/bin/make"
chmod 755 "$TC/p2/usr/local/bin/make"
sh "$here/toolchains.sh" "$TC/p2" "$TC/rh" 2>/dev/null | grep '^tool make ' >"$W/out2"
[ "$(cat "$W/out2")" = "tool make 4.4.1" ] && ok "toolchains.sh: a tool line for make" || bad "make tool line: $(cat "$W/out2")"
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
# pnpm is a tool of the same loop. helm has no `--version`: its line comes from
# `helm version --template {{.Version}}`, and a helm on PATH is read only with the prefix "".
printf '#!/bin/sh\necho 11.9.0\n' >"$TC/home/.local/bin/pnpm"
printf '#!/bin/sh\n[ "$1" = version ] && [ "$2" = --template ] && [ "$3" = "{{.Version}}" ] && printf v3.8.1 && exit 0\necho "Error: unknown flag" >&2\nexit 1\n' >"$TC/p3/usr/local/bin/helm"
chmod 755 "$TC/home/.local/bin/pnpm" "$TC/p3/usr/local/bin/helm"
HOME=$TC/home sh "$here/toolchains.sh" "$TC/p3" "$TC/rh" 2>/dev/null | grep '^tool pnpm\|^tool helm' | tr '\n' ',' >"$W/out2"
[ "$(cat "$W/out2")" = "tool pnpm 11.9.0,tool helm v3.8.1," ] && ok "toolchains.sh: pnpm and helm lines, helm through \`version --template\`" || bad "pnpm, helm lines: $(cat "$W/out2")"
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
for part in slot tool:wasm-bindgen tool:geckodriver tool:sccache tool:cargo-nextest tool:task tool:ess tool:cargo-about rustup archive:go archive:node archive:typescript archive:prettier archive:esbuild archive:pnpm archive:yq archive:helm make home:ess@0.97.4 home:@types/node@26.3.0 home:undici-types@8.3.0 home:aep@0.80.0 home:groovy@2.4.10; do
  grep -q "^failed: $part: " "$W/err" && ok "wrong SHA-256: $part fails" || bad "wrong SHA-256: no failed line for $part: $(grep '^failed' "$W/err" | head -3)"
done
[ ! -s "$W/tar.log" ] && ok "wrong SHA-256: tar never runs" || bad "tar ran on an unchecked file: $(cat "$W/tar.log")"
[ ! -e "$W/ran" ] && ok "wrong SHA-256: no downloaded file runs" || bad "a downloaded file ran"
[ "$(grep -c 'rustup-init\|go1.27.1\|node-v22\|task_linux\|nextest\|geckodriver\|wasm-bindgen\|sccache\|slot-0.36\|ess-0.99.2\|cargo-about-0.9.1\|yq_linux_amd64' "$W/curl.log")" -ge 12 ] && ok "wrong SHA-256: every download was attempted" || bad "curl calls: $(cat "$W/curl.log")"
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
# check --canary runs it; check alone starts no container.
crun() { PATH=$W/cn/bin:$PATH NODE_TEST=1 NODE_ROOT=$RR sh "$P" check "$W/node.conf" "$@" >"$W/out" 2>"$W/err"; }
printf 'canary: role slot-node-node-1\ncanary: answer AccessDeniedException\ncanary: end\n' >"$W/cn/answer"
echo 0 >"$W/cn/exit"
: >"$W/cn/calls"
crun
st=$?
[ $st -eq 0 ] && [ ! -s "$W/cn/calls" ] && ok "check without --canary starts no container" || bad "check started a container: exit $st $(cat "$W/cn/calls" "$W/out")"
crun --canary
st=$?
[ $st -eq 0 ] && grep -q '^canary: ok: role slot-node-node-1: ' "$W/out" && grep -q '^0 drift$' "$W/out" && [ "$(wc -l <"$W/cn/calls")" -eq 1 ] && ok "check --canary with a refused call: one container, one canary line, 0 drift" || bad "check --canary: exit $st $(cat "$W/out" "$W/err")"
printf 'canary: role slot-node-node-1\ncanary: answer ParameterNotFound\ncanary: end\n' >"$W/cn/answer"
crun --canary
st=$?
[ $st -eq 1 ] && grep -qx 'drift: role: canary: role slot-node-node-1 may read parameters: Systems Manager allowed ssm:GetParameter to a container' "$W/out" && ok "check --canary with an allowed call: drift, exit 1" || bad "check --canary allowed: exit $st $(cat "$W/out" "$W/err")"
rm -f "$RR/.state/active/docker.service"
: >"$W/cn/calls"
crun --canary
grep -qx 'drift: role: canary: docker.service is not active: nothing was tested' "$W/out" && [ ! -s "$W/cn/calls" ] && ok "check --canary without a running docker says that nothing was tested" || bad "canary without docker: $(cat "$W/out")"
: >"$RR/.state/active/docker.service"
sh "$P" bundle "$W/node.conf" check --canary >"$W/bundle-canary.sh"
grep -qx 'sh "$d/provision.sh" check "$d/node.conf" --canary' "$W/bundle-canary.sh" && ok "a check bundle carries --canary" || bad "bundle --canary: $(tail -4 "$W/bundle-canary.sh" | head -1)"

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

# 7. Nothing private in the repository's node files: no home of a real user, no cloud ids, no
# addresses but loopback and the metadata service.
leaks=$(grep -nE '/home/[a-z]|\b(i|vol|subnet|sg|vpc|ami)-0?[0-9a-f]{8,}\b|[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[a-z]{2,}|\b[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\b' \
  "$here"/*.sh "$here"/node.conf.example "$here"/README.md 2>/dev/null |
  grep -vE '169\.254\.169\.254|127\.0\.0\.1|0\.0\.0\.0' | grep -v 'test.sh:')
[ -z "$leaks" ] && ok "no private values in contrib/node" || bad "private values: $leaks"

echo "$fails failed"
[ $fails -eq 0 ]
