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

# 6c. push.sh apply keeps the setup login's ssh connection to itself and ends it on exit: a
# shared control connection outlives the key's removal from authorized_keys.
printf '#!/bin/sh\necho "$*" >>"%s"\nexit 1\n' "$W/ssh.log" >"$W/fake-ssh"
chmod 755 "$W/fake-ssh"
: >"$W/ssh.log"
conf
printf 'NODE_SSH=%s\nPUSH_HOST=node-setup\nRUST_TOOLCHAINS=none\n' "$W/fake-ssh" >>"$W/node.conf"
mkdir -p "$W/home"
HOME=$W/home sh "$U" apply "$W/node.conf" >"$W/out" 2>"$W/err"
n=$(grep -c . "$W/ssh.log")
[ "$n" -gt 0 ] && ok "apply used the configured ssh ($n calls)" || bad "apply made no ssh call: $(tail -3 "$W/err")"
grep -v -- '-o ControlPath=' "$W/ssh.log" | grep -q . && bad "an ssh call without a private control path: $(grep -v -- '-o ControlPath=' "$W/ssh.log" | head -1)" || ok "every ssh call names a private control path"
tail -1 "$W/ssh.log" | grep -q -- '-O exit node-setup' && ok "apply ends the control connection" || bad "last ssh call: $(tail -1 "$W/ssh.log")"

# 6d. push.sh apply copies only under the copy list the last root apply wrote on the node
# (/etc/slot-node/copies), and prints the list first.
grep -qx '.m2/repository go/bin' "$R/etc/slot-node/copies" 2>/dev/null && bad "copies written before the key exists" || :
conf
printf 'HOME_COPIES=.m2/repository go/bin\n' >>"$W/node.conf"
run "$R" apply
[ "$(cat "$R/etc/slot-node/copies" 2>/dev/null)" = ".m2/repository go/bin" ] && ok "apply writes the approved copy list" || bad "copies: $(cat "$R/etc/slot-node/copies" 2>&1)"
[ "$(stat -c %a "$R/etc/slot-node/copies" 2>/dev/null)" = 644 ] && ok "the copy list is readable by the build user" || bad "copies mode $(stat -c %a "$R/etc/slot-node/copies" 2>&1)"
printf '#!/bin/sh\necho "$*" >>"%s"\ncase "$*" in *"cat /etc/slot-node/copies"*) cat "%s"; exit 0 ;; esac\nexit 1\n' "$W/ssh.log" "$W/node-copies" >"$W/fake-ssh"
printf 'NODE_SSH=%s\nPUSH_HOST=node-setup\nRUST_TOOLCHAINS=none\n' "$W/fake-ssh" >>"$W/node.conf"
echo '.m2/repository go/bin .cache/other' >"$W/node-copies"
: >"$W/ssh.log"
HOME=$W/home sh "$U" apply "$W/node.conf" >"$W/out" 2>"$W/err"
st=$?
[ $st -eq 2 ] && grep -q 'approved copy list' "$W/err" && ok "another approved list: apply refuses" || bad "other list: exit $st $(cat "$W/err" | tail -2)"
grep -q 'rsync' "$W/ssh.log" && bad "copied under another list" || ok "nothing copied under another list"
echo '.m2/repository go/bin' >"$W/node-copies"
: >"$W/ssh.log"
HOME=$W/home sh "$U" apply "$W/node.conf" >"$W/out" 2>"$W/err"
grep -qx 'push: copy list: .m2/repository go/bin' "$W/out" && ok "apply prints the copy list" || bad "no list line: $(head -2 "$W/out")"
grep -q 'rsync' "$W/ssh.log" && ok "the same list: apply copies" || bad "same list, no copy: $(cat "$W/err" | tail -2)"

# 7. Nothing private in the repository's node files: no home of a real user, no cloud ids, no
# addresses but loopback and the metadata service.
leaks=$(grep -nE '/home/[a-z]|\b(i|vol|subnet|sg|vpc|ami)-0?[0-9a-f]{8,}\b|[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[a-z]{2,}|\b[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\b' \
  "$here"/*.sh "$here"/node.conf.example "$here"/README.md 2>/dev/null |
  grep -vE '169\.254\.169\.254|127\.0\.0\.1|0\.0\.0\.0' | grep -v 'test.sh:')
[ -z "$leaks" ] && ok "no private values in contrib/node" || bad "private values: $leaks"

echo "$fails failed"
[ $fails -eq 0 ]
