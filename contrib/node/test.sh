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
ETC_EXTRA=java21-openjdk
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
for item in storage binds packages user unit:slot-node.service unit:slot-storage.service keys sshd sudoers slot:/usr/local/bin/slot; do
  grep -q "^drift: $item" "$W/out" || bad "fresh machine: no drift line for $item"
done
run "$R" apply
st=$?
[ $st -eq 0 ] && ok "apply exits 0" || bad "apply exit $st: $(tail -3 "$W/err")"
grep -q '^pending: unit:slot-node.service' "$W/err" && ok "before the push, slot-node waits for the workstation /usr" || bad "no pending line: $(cat "$W/err")"
run "$R" check
grep -q '^drift: unit:slot-node.service: not active' "$W/out" && ok "before the push, check names slot-node" || bad "before the push: $(cat "$W/out")"
# What push.sh brings from the workstation: its /usr with the compile cache server, and /etc entries.
mkdir -p "$R/srv/slot/rootfs/usr/bin" && : >"$R/srv/slot/rootfs/usr/bin/sccache"
for e in fonts ssl ca-certificates ld.so.conf.d java21-openjdk; do mkdir -p "$R/srv/slot/rootfs/etc/$e"; done
for e in ld.so.cache localtime locale.conf nsswitch.conf ld.so.conf mime.types protocols services os-release; do : >"$R/srv/slot/rootfs/etc/$e"; done
run "$R" apply
run "$R" check
st=$?
if [ $st -eq 0 ] && grep -q '^0 drift$' "$W/out"; then ok "after apply: 0 drift"; else bad "after apply: exit $st, $(cat "$W/out")"; fi
grep -qx 'BindReadOnlyPaths=/srv/slot/rootfs/etc/java21-openjdk:/etc/java21-openjdk' "$R/etc/systemd/system/slot-node.service" && ok "ETC_EXTRA is bound" || bad "ETC_EXTRA not bound"
[ -d "$R/etc/java21-openjdk" ] && [ -f "$R/etc/localtime" ] && ok "bind targets have the type of their copy" || bad "bind target types"
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

# 6. The bundle is one self-contained script that gives the same answer.
sh "$P" bundle "$W/node.conf" check >"$W/bundle.sh" 2>"$W/err"
st=$?
if [ $st -eq 0 ] && sh -n "$W/bundle.sh"; then ok "bundle is valid sh"; else bad "bundle exit $st: $(cat "$W/err")"; fi
NODE_TEST=1 NODE_ROOT=$R sh "$W/bundle.sh" >"$W/out" 2>"$W/err"
st=$?
if [ $st -eq 0 ] && grep -q '^0 drift$' "$W/out"; then ok "bundle check: 0 drift"; else bad "bundle check exit $st: $(cat "$W/out" "$W/err" | tail -5)"; fi

# 6b. push.sh compares file by file: a file a node job added to a copied directory (a cache)
# is not drift; a file missing or of another size on the node is.
U=$here/push.sh
printf 'host ws\nbin cargo-x 10\ncopy .cache/t/a 5\ncopy .cache/t/b 7\n' >"$W/here"
printf 'host node\nbin cargo-x 10\ncopy .cache/t/a 5\ncopy .cache/t/b 7\ncopy .cache/t/job-added 3\n' >"$W/node"
sh "$U" compare "$W/here" "$W/node" >"$W/out" 2>"$W/err"
st=$?
[ $st -eq 0 ] && grep -q '^0 drift$' "$W/out" && ok "a file a job added on the node is not drift" || bad "job-added file: exit $st $(cat "$W/out" "$W/err")"
printf 'host node\nbin cargo-x 10\ncopy .cache/t/a 6\nbin cargo-y 3\n' >"$W/node"
sh "$U" compare "$W/here" "$W/node" >"$W/out" 2>"$W/err"
st=$?
[ $st -eq 1 ] && ok "compare with differences exits 1" || bad "compare with differences exit $st"
grep -qx 'drift: push: workstation has: copy .cache/t/a 5' "$W/out" && ok "a file of another size is drift" || bad "size: $(cat "$W/out")"
grep -qx 'drift: push: workstation has: copy .cache/t/b 7' "$W/out" && ok "a file missing on the node is drift" || bad "missing: $(cat "$W/out")"
grep -qx 'drift: push: only on the node: bin cargo-y 3' "$W/out" && ok "a cargo tool only on the node is drift" || bad "bin: $(cat "$W/out")"
grep -q '^3 drift$' "$W/out" && ok "three differences: 3 drift" || bad "count: $(cat "$W/out")"
printf 'host ws\n' >"$W/node"
sh "$U" compare "$W/here" "$W/node" >/dev/null 2>&1
[ $? -eq 2 ] && ok "a result from the workstation itself is refused" || bad "same host not refused"

# 7. Nothing private in the repository's node files: no home of a real user, no cloud ids, no
# addresses but loopback and the metadata service.
leaks=$(grep -nE '/home/[a-z]|\b(i|vol|subnet|sg|vpc|ami)-0?[0-9a-f]{8,}\b|[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[a-z]{2,}|\b[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\b' \
  "$here"/*.sh "$here"/node.conf.example "$here"/README.md 2>/dev/null |
  grep -vE '169\.254\.169\.254|127\.0\.0\.1|0\.0\.0\.0' | grep -v 'test.sh:')
[ -z "$leaks" ] && ok "no private values in contrib/node" || bad "private values: $leaks"

echo "$fails failed"
[ $fails -eq 0 ]
