#!/bin/sh
# provision.sh: brings a build node (Amazon Linux 2023 with NVMe instance storage) to the state a
# slot node needs, and reports where a node differs from it. Runs as root on the node.
#
#   provision.sh check CONF           print one `drift: <part>: <what>` line per difference,
#                                     then `<n> drift`; exit 0 with 0 drift, 1 otherwise
#   provision.sh apply CONF [--open]  change every part that drifts, print `changed: ...` per
#                                     change; --open also lets SETUP_KEY in (a setup login)
#   provision.sh storage CONF         the boot step of slot-storage.service: instance store
#                                     array, its mount, the job tmp filesystem and the binds
#   provision.sh bundle CONF MODE [--open]
#                                     print one self-contained script (this script, conf.sh,
#                                     toolchains.sh and CONF) that runs MODE, for a root channel
#                                     that takes one script, such as SSM Run Command
#   provision.sh pins                 print the cargo tools the node installs at a pinned
#                                     release, one `<binary> <version>` line each
#
# CONF is a node config file (KEY=value, see node.conf.example and README.md). A node starts as a
# bare Amazon Linux 2023 image and `apply` makes it complete: packages, rustup and the rust
# toolchains, the pinned tools, the slot release. Nothing is copied from the workstation. The
# instance store is empty after every stop and start; slot-storage.service rebuilds the array at
# boot and `apply` installs what lived on it again.
#
# Tests: NODE_ROOT prefixes every path, and NODE_TEST=1 makes the system queries and actions
# read and write files under NODE_ROOT/.state (test.sh).
set -u
# A bundle unpacks under umask 077 so its copy of the config stays private. Every file written here
# gets its mode by chmod; the directories this run creates get 755 from this umask.
umask 022
here=$(cd "$(dirname "$0")" && pwd)
. "$here/conf.sh"

R=${NODE_ROOT:-}
T=${NODE_TEST:-}
S=$R/.state
mode=${1:-}
cf=${2:-}
open=
case ${3:-} in --open) open=1 ;; esac

usage() {
  sed -n '2,17p' "$here/provision.sh" | sed 's/^# \{0,1\}//' >&2
  exit 2
}

# Tools the node installs at a pinned release into the store's cargo bin, the release its
# consumers' CI installs: a node job then runs the release its CI runs. One line each: name,
# version, SHA-256 of the release archive (.tar.gz), download directory (the version's directory
# under it, or with %s the version's directory itself), archive name (%s is the version), the
# binaries in it. The first line of each binary's `--version` starts with `<binary> <version>`.
# Not ess: it runs the release a repository pins exactly from its own digest-checked cache.
# geckodriver: the release on the forge runner image of the browser test lanes.
# sccache: the distribution has no package of it; the release the workstation runs.
TOOL_PINS='wasm-bindgen 0.2.130 9687f3bb551b9f66f37ed32a40d7913a00f7962db653054e56857bc306c3227f https://github.com/wasm-bindgen/wasm-bindgen/releases/download wasm-bindgen-%s-x86_64-unknown-linux-musl wasm-bindgen wasm-bindgen-test-runner wasm2es6js
geckodriver 0.37.1 e815130ea95983e162ae91843b48d3a3ce991735635fce83a647afde21e09f7e https://github.com/mozilla/geckodriver/releases/download/v%s geckodriver-v%s-linux64 geckodriver
sccache 0.18.0 45f1447fbe231e3037bde351ef70677dd212216c8d62ae7ca409fecc4d6acc89 https://github.com/mozilla/sccache/releases/download/v%s sccache-v%s-x86_64-unknown-linux-musl sccache
cargo-nextest 0.9.143 66786b9abe23920d022a182d1416b1bbc8130dd4872a9553d76985a1708dcd1e https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-%s cargo-nextest-%s-x86_64-unknown-linux-gnu cargo-nextest
task 3.54.0 680859dbb4d881a9c72d4d9a8f510825450849af8567deacd7302c01124416fb https://github.com/go-task/task/releases/download/v%s task_linux_amd64 task'
# Archives the node unpacks as root into /usr/local/<name>-<version> (the build user cannot
# replace them) and links into /usr/local/bin: the go and node releases the workstation runs, so
# that jobs resolve the same toolchain on both sides (a distribution package is older). One line
# each: name, version, SHA-256 of the archive, its URL, the leading path components to strip,
# the binaries to link (paths in the unpacked tree; the first one answers the version check:
# `go version` has a `go<version>` word, any other `--version` is the version; `path:name` links
# the binary at `path` as /usr/local/bin/<name>). Other releases: a consumer fetches its own
# (GOTOOLCHAIN=auto). typescript, prettier and esbuild are the npm tarballs of those releases
# (esbuild's is the native binary of its linux-x64 package), the releases the test lanes of the
# consumers read (`tsc`, `prettier` and `esbuild` on PATH).
ARCHIVE_PINS='go 1.27.1 63d339f0da5ab53635a56f2490a7984dfe12dfcff22ad749f63edaf590168445 https://go.dev/dl/go1.27.1.linux-amd64.tar.gz 1 bin/go bin/gofmt
node v22.23.3 1084aa36196bba4c3a5e69a1ee388a6e4ff729dad09445fbcd434b28fe3c24af https://nodejs.org/dist/v22.23.3/node-v22.23.3-linux-x64.tar.gz 1 bin/node bin/npm bin/npx bin/corepack
typescript 6.0.3 33cd0ee1beaa8c9e9d15a9da836c62ddea4c34a42d7c2d349dbc80d94165d22a https://registry.npmjs.org/typescript/-/typescript-6.0.3.tgz 1 bin/tsc bin/tsserver
prettier 3.8.1 5531dc6006ad06b642d5342438909f85dc53e87c50556753c908229b213fb4f4 https://registry.npmjs.org/prettier/-/prettier-3.8.1.tgz 1 bin/prettier.cjs:prettier
esbuild 0.28.2 9573bb2233aab0f9ea7647d5cca9726113cc1768de61d66b17267f4db84488f6 https://registry.npmjs.org/@esbuild/linux-x64/-/linux-x64-0.28.2.tgz 1 bin/esbuild'
RUSTUP_VERSION=1.29.3
RUSTUP_SHA256=b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55
RUSTUP_URL=https://static.rust-lang.org/rustup/archive/$RUSTUP_VERSION/x86_64-unknown-linux-gnu/rustup-init
RUST_TRIPLE=x86_64-unknown-linux-gnu
pins() {
  echo "$TOOL_PINS" | while read -r _ pv _ _ _ pb; do for b in $pb; do echo "$b $pv"; done; done
}
if [ "$mode" = pins ]; then
  pins
  exit 0
fi
[ -n "$mode" ] && [ -n "$cf" ] || usage

BASE_PACKAGES='mdadm xfsprogs e2fsprogs rsync gcc gcc-c++ make clang lld pkgconf-pkg-config openssl-devel git tar gzip cmake protobuf-compiler perl iptables-nft chrony zstd python3'
SSHD_LINES='MaxSessions 200
MaxStartups 100:30:200
ClientAliveInterval 15
ClientAliveCountMax 3'
UNITS='slot-storage.service slot-node.service sccache-node.service slot-imds-root-only.service slot-toolchains-manifest.service slot-toolchains-manifest.timer'
ENABLED='slot-storage.service slot-node.service sccache-node.service slot-imds-root-only.service slot-toolchains-manifest.timer'
LIBEXEC=/usr/local/libexec

if [ "$mode" = bundle ]; then
  conf_read "$cf" BUILD_USER BUILD_UID BUILD_HOME BUILD_KEY SLOT_VERSION || exit 2
  m=${3:-}
  case $m in check | apply) ;; *) usage ;; esac
  o=${4:-}
  b=$(mktemp -d "${TMPDIR:-/tmp}/slot-node-bundle.XXXXXX") || exit 1
  cp "$here/provision.sh" "$here/conf.sh" "$here/toolchains.sh" "$b/" && cp "$cf" "$b/node.conf" || exit 1
  blob=$(cd "$b" && tar -czf - provision.sh conf.sh toolchains.sh node.conf | base64 -w0)
  rm -rf "$b"
  printf '#!/bin/sh\n'
  printf '# slot node provisioning bundle: provision.sh %s %s (generated by provision.sh bundle)\n' "$m" "$o"
  printf 'umask 077\n'
  printf 'd=$(mktemp -d "${TMPDIR:-/tmp}/slot-node.XXXXXX") || exit 1\n'
  printf "printf '%%s' '%s' | base64 -d | tar -xzf - -C \"\$d\" || exit 1\n" "$blob"
  printf 'sh "$d/provision.sh" %s "$d/node.conf" %s\n' "$m" "$o"
  printf 'st=$?\nrm -rf "$d"\nexit $st\n'
  exit 0
fi

conf_read "$cf" BUILD_USER BUILD_UID BUILD_HOME BUILD_KEY SLOT_VERSION || exit 2
H=$BUILD_HOME
U=$BUILD_USER

ndrift=0
nchanged=0
nfailed=0

# A part that differs: reported in check mode; in apply mode the caller fixes it and says so.
drift() {
  if [ "$mode" = check ]; then echo "drift: $1: $2"; fi
  ndrift=$((ndrift + 1))
}
changed() {
  echo "changed: $1: $2"
  nchanged=$((nchanged + 1))
}
failed() {
  echo "failed: $1: $2" >&2
  nfailed=$((nfailed + 1))
}
fixing() { [ "$mode" = apply ] || [ "$mode" = storage ]; }

key() { echo "$1" | tr / _; }
calls() { [ -n "$T" ] && echo "$*" >>"$R/.calls"; }

# ---- system queries (NODE_TEST=1: files under $S) ----

q_mount() {
  if [ -n "$T" ]; then
    awk -v t="$1" '$3 == t { print $1, $2 }' "$S/mounts" 2>/dev/null | tail -1
  else
    findmnt -rn -M "$1" -o FSROOT,FSTYPE 2>/dev/null | tail -1
  fi
}
q_pkg() {
  if [ -n "$T" ]; then grep -qx "$1" "$S/packages" 2>/dev/null; else rpm -q --quiet "$1"; fi
}
q_user() {
  if [ -n "$T" ]; then
    awk -F: -v u="$1" '$1 == u { print $3, $6 }' "$S/passwd" 2>/dev/null
  else
    getent passwd "$1" | awk -F: '{ print $3, $6 }'
  fi
}
q_enabled() {
  if [ -n "$T" ]; then [ -f "$S/enabled/$1" ]; else systemctl is-enabled -q "$1" 2>/dev/null; fi
}
q_active() {
  if [ -n "$T" ]; then [ -f "$S/active/$1" ]; else systemctl is-active -q "$1" 2>/dev/null; fi
}
q_running_args() {
  if [ -n "$T" ]; then
    cat "$S/running/$1" 2>/dev/null
  else
    p=$(systemctl show -p MainPID --value "$1" 2>/dev/null)
    [ -n "$p" ] && [ "$p" != 0 ] && tr '\0' ' ' <"/proc/$p/cmdline" 2>/dev/null | sed 's/ *$//'
  fi
}
# The Environment= and BindReadOnlyPaths= settings of a unit file, one `Key=item` per line, sorted:
# what the manager loads from it and what a process of it must have started with.
unit_defs() {
  awk '/^(Environment|BindReadOnlyPaths)=/ { k = $0; sub(/=.*/, "", k); v = substr($0, length(k) + 2)
    n = split(v, a, " "); for (i = 1; i <= n; i++) print k "=" a[i] }' "$R/etc/systemd/system/$1" 2>/dev/null | sort
}
# The same settings as the manager has them loaded (`systemctl show`).
q_loaded() {
  if [ -n "$T" ]; then
    cat "$S/loaded/$1" 2>/dev/null
  else
    for qp in Environment BindReadOnlyPaths; do
      systemctl show -p "$qp" --value "$1" 2>/dev/null | tr ' ' '\n' | sed "/^\$/d;s/^/$qp=/"
    done | sort
  fi
}
# The environment of the running main process, as `Environment=NAME=value` lines.
q_procenv() {
  if [ -n "$T" ]; then
    cat "$S/procenv/$1" 2>/dev/null
  else
    qm=$(systemctl show -p MainPID --value "$1" 2>/dev/null)
    [ -n "$qm" ] && [ "$qm" != 0 ] && tr '\0' '\n' <"/proc/$qm/environ" 2>/dev/null | sed 's/^/Environment=/'
  fi
}
# The version line of the slot binary the running unit executes (not the file at the path now).
q_running_version() {
  if [ -n "$T" ]; then
    cat "$S/running-version/$1" 2>/dev/null
  else
    qm=$(systemctl show -p MainPID --value "$1" 2>/dev/null)
    [ -n "$qm" ] && [ "$qm" != 0 ] && as_user "/proc/$qm/exe" --version 2>/dev/null </dev/null | head -n 1
  fi
}
q_jobs() {
  if [ -n "$T" ]; then
    cat "$S/jobs" 2>/dev/null || echo 0
  else
    runuser -u "$U" -- env XDG_RUNTIME_DIR="/run/user/$BUILD_UID" "$STORE/bin/slot" status 2>/dev/null |
      head -1 | sed -n 's/.* \([0-9]*\) running, \([0-9]*\) queued.*/\1 \2/p' | awk '{ print $1 + $2 }'
  fi
}
q_immutable() {
  if [ -n "$T" ]; then
    [ -f "$S/immutable/$(key "$R$1")" ]
  else
    case $(lsattr -d "$1" 2>/dev/null) in ????i*) return 0 ;; *) return 1 ;; esac
  fi
}
q_imds() {
  if [ -n "$T" ]; then
    [ -f "$S/imds" ]
  else
    iptables -w -C OUTPUT -d 169.254.169.254/32 -j SLOT-IMDS 2>/dev/null &&
      ip6tables -w -C OUTPUT -d fd00:ec2::254/128 -j SLOT-IMDS 2>/dev/null
  fi
}
q_nproc() {
  if [ -n "$T" ]; then cat "$R/.state-nproc" 2>/dev/null || echo 2; else nproc; fi
}
q_owner() { stat -c %U "$R$1" 2>/dev/null; }

# ---- system actions ----

st_add() {
  mkdir -p "$S"
  echo "$2" >>"$S/$1"
}
st_touch() {
  mkdir -p "$S/$1"
  : >"$S/$1/$2"
}
a_run() {
  calls "$*"
  [ -n "$T" ] || "$@"
}
a_daemon_reload() { a_run systemctl daemon-reload; }
a_enable() {
  if [ -n "$T" ]; then st_touch enabled "$1"; else systemctl enable -q "$1"; fi
}
a_start() {
  if [ -n "$T" ]; then
    st_touch active "$1"
    mkdir -p "$S/running" "$S/loaded" "$S/procenv" "$S/running-version"
    sed -n 's/^ExecStart=//p' "$R/etc/systemd/system/$1" >"$S/running/$1"
    unit_defs "$1" >"$S/loaded/$1"
    unit_defs "$1" | grep '^Environment=' >"$S/procenv/$1"
    [ "$1" = slot-node.service ] && "$R$STORE/bin/slot" --version >"$S/running-version/$1" 2>/dev/null
    return 0
  else
    systemctl start "$1"
  fi
}
a_restart() {
  if [ -n "$T" ]; then
    calls "systemctl restart $1"
    a_start "$1"
  else
    systemctl restart "$1"
  fi
}
a_chattr() {
  if [ -n "$T" ]; then
    case $1 in
      +i) st_touch immutable "$(key "$R$2")" ;;
      -i) rm -f "$S/immutable/$(key "$R$2")" ;;
    esac
  else
    chattr "$1" "$2"
  fi
}
a_chown() {
  [ -n "$T" ] && return 0
  chown -h "$@"
}
# The ssh logins of this boot with the key of fingerprint $1 that still run, one line each:
# `session <id> <leader pid>` or, without a logind session, `pid <pid>`.
# The process that holds a connection logs "Accepted publickey ... <fingerprint>": as
# sshd[<pid>] up to OpenSSH 9.7, as sshd-session[<pid>] from 9.8. With pam_systemd that process
# leads a logind session whose scope holds every process the login started, also one whose
# parent became pid 1, and the session stays while one of them runs. A session counts when its
# leader is that pid and it began within 2 s of the line; a bare pid counts while its process is
# sshd or sshd-session and started no later than the line. So a pid number reused by a later
# connection or another program is left alone.
q_key_logins() {
  if [ -n "$T" ]; then sdir=$S/sessions; else sdir=/run/systemd/sessions; fi
  if [ -n "$T" ]; then
    cat "$S/sshd-journal" 2>/dev/null
  else
    journalctl -b -q --no-pager -o short-unix -t sshd -t sshd-session 2>/dev/null
  fi |
    awk -v fp="$1" '/sshd(-session)?\[[0-9]+\]: Accepted publickey for / && $NF == fp {
      s = $0; sub(/.*sshd(-session)?\[/, "", s); sub(/\].*/, "", s)
      t = $1; sub(/\..*/, "", t); print s, t }' | sort -un |
    while read -r pid t; do
      sid=
      for f in "$sdir"/*; do
        case ${f##*/} in *[!0-9a-z]* | "*") continue ;; esac
        [ -f "$f" ] || continue
        l=$(sed -n 's/^LEADER=//p' "$f")
        r=$(sed -n 's/^REALTIME=//p' "$f")
        [ "$l" = "$pid" ] && [ -n "$r" ] || continue
        d=$((r / 1000000 - t))
        [ "$d" -ge -2 ] && [ "$d" -le 2 ] && sid=${f##*/}
      done
      if [ -n "$sid" ]; then
        echo "session $sid $pid"
        continue
      fi
      if [ -n "$T" ]; then
        set -- $(awk -v p="$pid" '$1 == p { print $2, $3 }' "$S/alive-pids" 2>/dev/null)
        comm=${1:-} st=${2:-}
      else
        comm=$(cat "/proc/$pid/comm" 2>/dev/null)
        ticks=$(awk '{ print $22 }' "/proc/$pid/stat" 2>/dev/null)
        st=
        [ -n "$ticks" ] && st=$(($(awk '/^btime/ { print $2 }' /proc/stat) + ticks / $(getconf CLK_TCK)))
      fi
      case $comm in sshd | sshd-session) ;; *) continue ;; esac
      [ -n "$st" ] && [ "$st" -le $((t + 1)) ] && echo "pid $pid"
    done
}
# Ends a login's logind session: systemd stops its scope, which ends every process in it (the
# connection's processes and every command the login started, with SIGTERM, then SIGKILL).
a_end_session() {
  if [ -n "$T" ]; then
    calls "terminate-session $1"
    for p in $(cat "$S/session-procs/$1" 2>/dev/null); do
      awk -v p="$p" '$1 != p' "$S/alive-pids" >"$S/alive-pids.n" 2>/dev/null
      mv "$S/alive-pids.n" "$S/alive-pids"
    done
    rm -f "$S/sessions/$1" "$S/session-procs/$1"
  else
    loginctl terminate-session "$1"
  fi
}
# Ends a connection without a logind session: the process that logged the login. Commands the
# login started can outlive it; only a session ends those.
a_kill() {
  if [ -n "$T" ]; then
    calls "kill $1"
    awk -v p="$1" '$1 != p' "$S/alive-pids" >"$S/alive-pids.n" 2>/dev/null
    mv "$S/alive-pids.n" "$S/alive-pids"
  else
    kill "$1"
  fi
}

# Jobs run as the build user, so root never follows a link that user could have planted: a path
# in the user's area must have no symbolic link in it, and the user's files are written by the
# user (as_user). Directly under STORE only root can create or replace entries.
no_link() {
  nl=
  [ -n "$R" ] && nl=$(readlink -m "$R")
  [ "$(readlink -m "$R$1")" = "$nl$1" ]
}
user_path() { echo "$STORE/nodebin:$H/.cargo/bin:/usr/local/bin:/usr/bin:/bin"; }
# A command of the build user runs with a clean environment (nothing of root's or of this run's
# variables), the units' PATH, and its own session when setpriv can make one.
as_user() {
  if [ -n "$T" ]; then
    env -i HOME="$H" PATH="$(user_path)" LANG=C.UTF-8 "$@"
  elif command -v setpriv >/dev/null 2>&1 && setpriv --help 2>&1 | grep -q -- --setsid; then
    runuser -u "$U" -- setpriv --setsid env -i HOME="$H" PATH="$(user_path)" LANG=C.UTF-8 "$@"
  else
    runuser -u "$U" -- env -i HOME="$H" PATH="$(user_path)" LANG=C.UTF-8 "$@"
  fi
}
# NODE_TEST_REAL_FETCH=1 (test.sh): the download functions take their real branch under NODE_TEST.
fake_fetch() { [ -n "$T" ] && [ -z "${NODE_TEST_REAL_FETCH:-}" ]; }
root_name() {
  if [ -n "$T" ]; then id -un; else echo root; fi
}

# ---- expected content ----

tokens() {
  if [ -n "$TOKENS" ]; then echo "$TOKENS"; else q_nproc; fi
}
serve_args() {
  echo "$STORE/bin/slot serve --tokens $(tokens) --min-free-disk $MIN_FREE_DISK --same-path-root $H"
}
go_env() {
  [ -n "$GO_PRIVATE" ] && printf ' GOPRIVATE=%s GONOSUMDB=%s' "$GO_PRIVATE" "$GO_PRIVATE"
  return 0
}
gen_unit() {
  case $1 in
    slot-storage.service)
      cat <<EOF
[Unit]
Description=slot node storage: instance store array, job tmp filesystem and binds
DefaultDependencies=no
After=local-fs.target
Before=sccache-node.service slot-node.service
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/bin/sh $LIBEXEC/slot-node/provision.sh storage /etc/slot-node/node.conf
[Install]
WantedBy=multi-user.target
EOF
      ;;
    slot-node.service)
      cat <<EOF
[Unit]
Description=slot build pool
After=network-online.target local-fs.target slot-storage.service sccache-node.service
Requires=slot-storage.service
[Service]
User=$U
Group=$U
Environment=HOME=$H XDG_RUNTIME_DIR=/run/user/$BUILD_UID
Environment=PATH=$STORE/nodebin:$H/.cargo/bin:/usr/local/bin:/usr/bin:$H/.bun/bin:$H/go/bin:$H/.local/bin:/bin GIT_TERMINAL_PROMPT=0 GIT_ASKPASS=$STORE/nodebin/no-credentials SSH_ASKPASS=$STORE/nodebin/no-credentials TMPDIR=$STORE/tmp$(go_env)
ExecStart=$(serve_args)
ExecReload=/bin/kill -HUP \$MAINPID
KillMode=control-group
LimitNOFILE=65536
Restart=always
[Install]
WantedBy=multi-user.target
EOF
      ;;
    sccache-node.service)
      cat <<EOF
[Unit]
Description=sccache for slot-node jobs
After=slot-storage.service
Requires=slot-storage.service
Before=slot-node.service
[Service]
User=$U
Group=$U
Environment=HOME=$H XDG_RUNTIME_DIR=/run/user/$BUILD_UID SCCACHE_NO_DAEMON=1 SCCACHE_START_SERVER=1 SCCACHE_IDLE_TIMEOUT=0
Environment=PATH=$STORE/nodebin:$H/.cargo/bin:/usr/local/bin:/usr/bin:/bin TMPDIR=$STORE/tmp
ExecStart=$H/.cargo/bin/sccache
LimitNOFILE=65536
Restart=always
[Install]
WantedBy=multi-user.target
EOF
      ;;
    slot-imds-root-only.service)
      cat <<'EOF'
[Unit]
Description=Only root reaches the instance metadata service
DefaultDependencies=no
Before=network-pre.target slot-node.service
Wants=network-pre.target

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=-/usr/sbin/iptables -w -N SLOT-IMDS
ExecStart=/usr/sbin/iptables -w -F SLOT-IMDS
ExecStart=/usr/sbin/iptables -w -A SLOT-IMDS -m owner --uid-owner 0 -j RETURN
ExecStart=/usr/sbin/iptables -w -A SLOT-IMDS -j REJECT
ExecStart=/bin/sh -c '/usr/sbin/iptables -w -C OUTPUT -d 169.254.169.254/32 -j SLOT-IMDS 2>/dev/null || /usr/sbin/iptables -w -I OUTPUT 1 -d 169.254.169.254/32 -j SLOT-IMDS'
ExecStart=-/usr/sbin/ip6tables -w -N SLOT-IMDS
ExecStart=/usr/sbin/ip6tables -w -F SLOT-IMDS
ExecStart=/usr/sbin/ip6tables -w -A SLOT-IMDS -m owner --uid-owner 0 -j RETURN
ExecStart=/usr/sbin/ip6tables -w -A SLOT-IMDS -j REJECT
ExecStart=/bin/sh -c '/usr/sbin/ip6tables -w -C OUTPUT -d fd00:ec2::254/128 -j SLOT-IMDS 2>/dev/null || /usr/sbin/ip6tables -w -I OUTPUT 1 -d fd00:ec2::254/128 -j SLOT-IMDS'
ExecStop=-/usr/sbin/iptables -w -D OUTPUT -d 169.254.169.254/32 -j SLOT-IMDS
ExecStop=-/usr/sbin/iptables -w -F SLOT-IMDS
ExecStop=-/usr/sbin/iptables -w -X SLOT-IMDS
ExecStop=-/usr/sbin/ip6tables -w -D OUTPUT -d fd00:ec2::254/128 -j SLOT-IMDS
ExecStop=-/usr/sbin/ip6tables -w -F SLOT-IMDS
ExecStop=-/usr/sbin/ip6tables -w -X SLOT-IMDS

[Install]
WantedBy=multi-user.target
EOF
      ;;
    slot-toolchains-manifest.service)
      cat <<EOF
[Unit]
Description=Toolchain manifest of this node for the workstation's parity check

[Service]
Type=oneshot
User=$U
ExecStart=/bin/sh -c 'd=$H/.local/state/slot; /bin/sh $LIBEXEC/slot-toolchains.sh "" $STORE/rustup > \$d/toolchains-node.txt.tmp && [ -s \$d/toolchains-node.txt.tmp ] && mv \$d/toolchains-node.txt.tmp \$d/toolchains-node.txt'
EOF
      ;;
    slot-toolchains-manifest.timer)
      cat <<'EOF'
[Unit]
Description=Toolchain manifest every 60 s

[Timer]
OnBootSec=30
OnUnitActiveSec=60
AccuracySec=5

[Install]
WantedBy=timers.target
EOF
      ;;
  esac
}
gen_keys() {
  echo "command=\"/usr/local/bin/slot shell --same-path-root $H\",no-port-forwarding,no-agent-forwarding,no-X11-forwarding $BUILD_KEY"
  [ -n "$open" ] && [ -n "$SETUP_KEY" ] && echo "$SETUP_KEY"
  return 0
}
gen_sudoers() {
  echo "$U ALL=(root) NOPASSWD: /usr/bin/systemctl reload slot-node, /usr/bin/systemctl restart slot-node, /usr/bin/systemctl status slot-node"
}
gen_cargo_config() {
  cat <<'EOF'
[build]
rustc-wrapper = "sccache"
incremental = false
[profile.dev]
debug = false
[target.x86_64-unknown-linux-gnu]
rustflags = ["-C", "link-arg=-fuse-ld=lld"]
EOF
}
gen_sccache_config() {
  printf '[cache.disk]\ndir = "%s/sccache"\nsize = %s\n' "$STORE" "$((SCCACHE_GB * 1073741824))"
}
gen_no_credentials() {
  cat <<EOF
#!/bin/sh
echo "slot: $NODE_NAME has no credentials (rule: no credential goes to the node). Clone or fetch on the workstation before the job. Use slot --here only for the step that reads a credential while it runs (a clone, a fetch, a push), never for a whole task: the rest of the task builds on the node. A step that needs docker or a local service goes in .slot.toml local = [...] of its repository." >&2
exit 255
EOF
}
gen_ssh_shim() {
  cat <<EOF
#!/bin/sh
# ssh on $NODE_NAME: the node has no credentials, so every ssh that would connect is refused.
# Let through only the calls that open no connection and read no key: -G (print the resolved
# configuration and exit) and -V (print the version).
take=
for a in "\$@"; do
  if [ -n "\$take" ]; then take=; continue; fi
  case \$a in
    --) break ;;
    -[BbcDEeFIiJLlmOoPpQRSWw]) take=1 ;;
    -*)
      f=\${a#-}
      case \$f in *[BbcDEeFIiJLlmOoPpQRSWw]*) f=\${f%%[BbcDEeFIiJLlmOoPpQRSWw]*} ;; esac
      case \$f in *G* | *V*) exec /usr/bin/ssh "\$@" ;; esac
      ;;
    *) break ;;
  esac
done
echo "slot: $NODE_NAME has no credentials (rule: no credential goes to the node). Clone or fetch on the workstation before the job. Use slot --here only for the step that reads a credential while it runs (a clone, a fetch, a push), never for a whole task: the rest of the task builds on the node. A step that needs docker or a local service goes in .slot.toml local = [...] of its repository." >&2
exit 255
EOF
}

# want_file PART PATH MODE OWNER GENERATOR...: the file has the generator's bytes.
want_file() {
  part=$1 path=$2 fmode=$3 owner=$4
  shift 4
  tmp=$(mktemp) || return 1
  "$@" >"$tmp"
  if [ -f "$R$path" ] && cmp -s "$tmp" "$R$path"; then
    rm -f "$tmp"
    return 0
  fi
  if [ -f "$R$path" ]; then why="content differs"; else why="missing"; fi
  drift "$part" "$path $why"
  if fixing; then
    if ! no_link "$path"; then
      failed "$part" "$path has a symbolic link in it; not written"
    elif [ "$owner" = "$U:$U" ]; then
      as_user sh -c 'mkdir -p "$(dirname "$1")" && { [ ! -f "$1" ] || cp -p "$1" "$1.slot-prev"; } &&
        cat >"$1.slot-new" && chmod "$2" "$1.slot-new" && mv "$1.slot-new" "$1"' sh "$R$path" "$fmode" <"$tmp" &&
        changed "$part" "$path written"
    else
      mkdir -p "$(dirname "$R$path")"
      [ -f "$R$path" ] && cp -p "$R$path" "$R$path.slot-prev"
      cp "$tmp" "$R$path.slot-new" && chmod "$fmode" "$R$path.slot-new" &&
        a_chown "$owner" "$R$path.slot-new" && mv "$R$path.slot-new" "$R$path" &&
        changed "$part" "$path written"
    fi
  fi
  rm -f "$tmp"
  return 1
}

# ---- parts ----

p_packages() {
  missing=
  for p in $BASE_PACKAGES $EXTRA_PACKAGES; do q_pkg "$p" || missing="$missing $p"; done
  [ -z "$missing" ] && return 0
  drift packages "not installed:$missing"
  if fixing; then
    if [ -n "$T" ]; then
      for p in $missing; do st_add packages "$p"; done
    else
      dnf -y -q install $missing >/dev/null || failed packages "dnf install$missing"
    fi
    changed packages "installed$missing"
  fi
}

p_user() {
  got=$(q_user "$U")
  if [ -z "$got" ]; then
    drift user "$U is missing"
    if fixing; then
      if [ -n "$T" ]; then
        st_add passwd "$U:x:$BUILD_UID:$BUILD_UID::$H:/bin/bash"
        mkdir -p "$R$H"
      else
        useradd -m -u "$BUILD_UID" -d "$H" -s /bin/bash "$U" || failed user "useradd $U"
      fi
      changed user "$U created with uid $BUILD_UID"
    fi
  elif [ "$got" != "$BUILD_UID $H" ]; then
    drift user "$U is uid and home '$got', config says '$BUILD_UID $H'"
    fixing && failed user "an existing user is never changed; fix the config or the user by hand"
  fi
  if [ ! -f "$R/var/lib/systemd/linger/$U" ]; then
    drift user "no linger for $U"
    if fixing; then
      if [ -n "$T" ]; then
        mkdir -p "$R/var/lib/systemd/linger" && : >"$R/var/lib/systemd/linger/$U"
      else
        loginctl enable-linger "$U"
      fi
      changed user "linger for $U"
    fi
  fi
  if ! grep -qx 'umask 022' "$R$H/.bashrc" 2>/dev/null; then
    drift user "$H/.bashrc has no umask 022"
    if fixing; then
      if no_link "$H/.bashrc"; then
        as_user sh -c 'echo "umask 022" >>"$1"' sh "$R$H/.bashrc" && changed user "umask 022 in $H/.bashrc"
      else
        failed user "$H/.bashrc has a symbolic link in it; not changed"
      fi
    fi
  fi
}

p_sshd() {
  f=$R/etc/ssh/sshd_config
  miss=$(echo "$SSHD_LINES" | while IFS= read -r l; do grep -qx "$l" "$f" 2>/dev/null || echo "$l"; done)
  [ -z "$miss" ] && return 0
  drift sshd "missing in /etc/ssh/sshd_config: $(echo "$miss" | tr '\n' ';')"
  if fixing; then
    mkdir -p "$(dirname "$f")"
    echo "$miss" >>"$f"
    a_run systemctl reload sshd
    changed sshd "$(echo "$miss" | wc -l) lines added"
  fi
}

# The instance store as one xfs filesystem at STORE: an md raid0 of every instance store disk,
# or the one disk. Never formats a device that holds a filesystem or is in use.
storage_array() {
  if [ -n "$T" ]; then
    st_add mounts "/ xfs $STORE"
    calls "mount store $STORE"
    return 0
  fi
  mdadm --assemble --scan >/dev/null 2>&1
  dev=
  [ -b /dev/md0 ] && dev=/dev/md0
  if [ -z "$dev" ]; then
    disks=$(lsblk -dpno NAME,MODEL | awk -v m="$DISK_MODEL" 'index($0, m) { print $1 }')
    n=$(echo $disks | wc -w)
    if [ "$n" -eq 0 ]; then
      failed storage "no disk with model '$DISK_MODEL'"
      return 1
    fi
    for d in $disks; do
      if [ -n "$(lsblk -no MOUNTPOINT "$d" | tr -d '[:space:]')" ]; then
        failed storage "$d is in use"
        return 1
      fi
    done
    if [ "$n" -gt 1 ]; then
      mdadm --create /dev/md0 --level=0 --raid-devices="$n" $disks --run >/dev/null || {
        failed storage "mdadm --create"
        return 1
      }
      dev=/dev/md0
    else
      dev=$disks
    fi
  fi
  if [ -z "$(blkid -p -o value -s TYPE "$dev" 2>/dev/null)" ]; then
    mkfs.xfs -q -f "$dev" || {
      failed storage "mkfs.xfs $dev"
      return 1
    }
  fi
  mkdir -p "$STORE"
  mount -o noatime "$dev" "$STORE" || {
    failed storage "mount $dev $STORE"
    return 1
  }
}

storage_tmp() {
  img=$STORE/tmp-ext4.img
  mkdir -p "$R$STORE/tmp"
  if [ -n "$T" ]; then
    : >"$R$img"
    st_add mounts "/ ext4 $STORE/tmp"
    return 0
  fi
  if [ -L "$img" ] || [ -L "$STORE/tmp" ]; then
    failed storage "$img or $STORE/tmp is a symbolic link"
    return 1
  fi
  if [ ! -f "$img" ]; then
    truncate -s "${TMP_GB}G" "$img" && mkfs.ext4 -q -F -m 0 -L slot-tmp "$img" || {
      failed storage "tmp image"
      return 1
    }
  fi
  mount -o loop,noatime "$img" "$STORE/tmp" || {
    failed storage "mount $img"
    return 1
  }
}

p_storage() {
  m=$(q_mount "$STORE")
  case $m in
    *" xfs") ;;
    *)
      drift storage "$STORE is not an xfs mount (${m:-not mounted})"
      if fixing; then storage_array && changed storage "$STORE mounted"; fi
      ;;
  esac
  m=$(q_mount "$STORE/tmp")
  case $m in
    *" ext4") ;;
    *)
      drift storage "$STORE/tmp is not the ext4 job tmp (${m:-not mounted})"
      if fixing; then storage_tmp && changed storage "$STORE/tmp mounted"; fi
      ;;
  esac
}

# STORE and STORE/home belong to root, so only root creates or replaces their entries (the tmp
# image among them); everything below belongs to the build user.
p_dirs() {
  rn=$(root_name)
  for p in "$STORE" "$STORE/home"; do
    if [ ! -d "$R$p" ] || [ -L "$R$p" ]; then
      drift dirs "$p missing"
      if fixing; then mkdir -p "$R$p" && chmod 755 "$R$p" && changed dirs "$p created"; fi
    elif [ "$(q_owner "$p")" != "$rn" ]; then
      drift dirs "$p is owned by $(q_owner "$p"), not root"
      if fixing; then a_chown root:root "$R$p" && chmod 755 "$R$p" && changed dirs "$p owner root"; fi
    fi
  done
  for d in bin cargo nodebin rustup sccache state tmp $(for m in $MIRROR_DIRS; do echo "home/$m"; done); do
    p=$STORE/$d
    if [ ! -d "$R$p" ]; then
      drift dirs "$p missing"
      if fixing; then
        case $d in
          home/*/*)
            first=$STORE/home/$(echo "${d#home/}" | cut -d/ -f1)
            [ -d "$R$first" ] || { mkdir "$R$first" && a_chown "$U:$U" "$R$first"; }
            if no_link "$p"; then
              as_user mkdir -p "$R$p" && changed dirs "$p created"
            else
              failed dirs "$p has a symbolic link in it"
            fi
            ;;
          *) mkdir -p "$R$p" && a_chown "$U:$U" "$R$p" && changed dirs "$p created" ;;
        esac
      fi
    elif [ "$(q_owner "$p")" != "$U" ]; then
      drift dirs "$p is owned by $(q_owner "$p")"
      if fixing; then
        if no_link "$p"; then a_chown "$U:$U" "$R$p" && changed dirs "$p owner $U"; else failed dirs "$p has a symbolic link in it"; fi
      fi
    fi
  done
}

bind_pairs() {
  echo "/state .local/state"
  for m in $MIRROR_DIRS; do echo "/home/$m $m"; done
}
p_binds() {
  bt=$(mktemp) || return 1
  bind_pairs | while read -r src dst; do
    t=$H/$dst
    got=$(q_mount "$t")
    case $got in
      "$src "*) ;;
      *)
        echo "$t ${got:-not mounted}"
        ;;
    esac
  done >"$bt" 2>/dev/null || :
  if [ -s "$bt" ]; then
    while read -r t got; do
      drift binds "$t is ${got} (want $STORE$(bind_pairs | awk -v d="${t#"$H"/}" '$2 == d { print $1 }'))"
    done <"$bt"
    if fixing; then
      bind_pairs | while read -r src dst; do
        t=$H/$dst
        case $(q_mount "$t") in "$src "*) continue ;; esac
        if ! no_link "$STORE$src" || ! no_link "$t"; then
          echo "failed: binds: $STORE$src or $t has a symbolic link in it; not mounted" >&2
          continue
        fi
        [ -d "$R$STORE$src" ] || as_user mkdir -p "$R$STORE$src"
        [ -d "$R$t" ] || as_user mkdir -p "$R$t"
        if [ -n "$T" ]; then
          st_add mounts "$src xfs $t"
        else
          mount --bind "$STORE$src" "$t" || {
            echo "failed: binds: mount --bind $STORE$src $t" >&2
            continue
          }
        fi
        echo "changed: binds: $t from $STORE$src"
      done
      nchanged=$((nchanged + 1))
    fi
  fi
  rm -f "$bt"
}

p_fstab() {
  f=$R/etc/fstab
  [ -f "$f" ] || return 0
  bad=$(awk -v s="$STORE/" 'index($1, s) == 1' "$f")
  [ -z "$bad" ] && return 0
  drift fstab "$(echo "$bad" | wc -l) line(s) mount from $STORE, which is empty at boot; slot-storage.service does them"
  if fixing; then
    cp -p "$f" "$f.slot-prev"
    awk -v s="$STORE/" 'index($1, s) != 1' "$f.slot-prev" >"$f.slot-new" && cat "$f.slot-new" >"$f" && rm -f "$f.slot-new"
    changed fstab "$(echo "$bad" | wc -l) line(s) removed (old file: /etc/fstab.slot-prev)"
  fi
}

p_links() {
  for l in cargo rustup; do
    p=$H/.$l
    if [ "$(readlink "$R$p" 2>/dev/null)" != "$STORE/$l" ]; then
      drift "link:$p" "should point to $STORE/$l"
      if fixing; then
        if [ -e "$R$p" ] && [ ! -L "$R$p" ]; then
          failed "link:$p" "is a real directory; not replaced"
        else
          rm -f "$R$p" && as_user ln -s "$STORE/$l" "$R$p" && changed "link:$p" "-> $STORE/$l"
        fi
      fi
    fi
  done
}

p_files() {
  want_file cargo-config "$STORE/cargo/config.toml" 644 "$U:$U" gen_cargo_config
  want_file sccache-config "$H/.config/sccache/config" 644 "$U:$U" gen_sccache_config
  want_file nodebin "$STORE/nodebin/no-credentials" 755 "$U:$U" gen_no_credentials
  want_file nodebin "$STORE/nodebin/ssh" 755 "$U:$U" gen_ssh_shim
  want_file sudoers /etc/sudoers.d/slot-node 440 root:root gen_sudoers
  want_file libexec "$LIBEXEC/slot-toolchains.sh" 755 root:root cat "$here/toolchains.sh"
  want_file libexec "$LIBEXEC/slot-node/provision.sh" 755 root:root cat "$here/provision.sh"
  want_file libexec "$LIBEXEC/slot-node/conf.sh" 644 root:root cat "$here/conf.sh"
  want_file libexec /etc/slot-node/node.conf 600 root:root cat "$cf"
}

units_changed=
slot_replaced=
p_units() {
  for u in $UNITS; do
    if ! want_file "unit:$u" "/etc/systemd/system/$u" 644 root:root gen_unit "$u"; then
      units_changed="$units_changed $u"
    fi
  done
  if fixing && [ -n "$units_changed" ]; then a_daemon_reload; fi
  for u in $ENABLED; do
    q_enabled "$u" && continue
    drift "unit:$u" "not enabled"
    if fixing; then a_enable "$u" && changed "unit:$u" "enabled"; fi
  done
}

slot_paths() { echo "$STORE/bin/slot /usr/local/bin/slot $H/.local/bin/slot"; }
# Downloads the release into directory $1 and prints the path of its binary. NODE_TEST=1: a stub
# binary, or nothing when $R/.state-slot-fetch-fails exists.
slot_fetch() {
  a=slot-$SLOT_VERSION-x86_64-unknown-linux-gnu
  if fake_fetch; then
    [ -e "$R/.state-slot-fetch-fails" ] && return 0
    mkdir -p "$1/$a" && printf '#!/bin/sh\necho slot %s\n' "$SLOT_VERSION" >"$1/$a/slot" && chmod 755 "$1/$a/slot"
    calls "fetch slot $SLOT_VERSION"
  else
    u=https://github.com/beyond10x/slot/releases/download/$SLOT_VERSION
    (cd "$1" && curl -fsSLO "$u/$a.tar.gz" && curl -fsSLO "$u/SHA256SUMS" &&
      grep " $a.tar.gz\$" SHA256SUMS | sha256sum -c --strict - >/dev/null &&
      tar -xzf "$a.tar.gz" --no-same-owner && chmod -R u-s,g-s "$a") || return 0
  fi
  echo "$1/$a/slot"
}
# Installs the release binary at each path given and reads each back: returns 1 when a path does
# not answer `slot $SLOT_VERSION` afterwards.
install_slot() {
  if [ -n "$T" ]; then d=$(mktemp -d); else d=$(mktemp -d /var/tmp/slot-$SLOT_VERSION.XXXXXX); fi || return 1
  sb=$(slot_fetch "$d")
  if [ -z "$sb" ] || [ ! -x "$sb" ]; then
    rm -rf "$d"
    failed slot "download or checksum of slot $SLOT_VERSION"
    return 1
  fi
  chmod 755 "$d" "$(dirname "$sb")"
  rc=0
  for p in "$@"; do
    case $p in
      "$STORE"/* | "$H"/*)
        if ! no_link "$p"; then
          failed slot "$p has a symbolic link in it; not installed"
          rc=1
          continue
        fi
        as_user sh -c 'mkdir -p "$(dirname "$2")" && install -m 0755 "$1" "$2.slot-new" && mv "$2.slot-new" "$2"' sh "$sb" "$R$p"
        ;;
      *)
        mkdir -p "$(dirname "$R$p")"
        install -m 0755 "$sb" "$R$p.slot-new" && { [ -n "$T" ] || chown root:root "$p.slot-new"; } && mv "$R$p.slot-new" "$R$p"
        ;;
    esac
    v=$(as_user "$R$p" --version 2>/dev/null)
    if [ "$v" != "slot $SLOT_VERSION" ]; then
      failed slot "$p is '${v:-missing}' after the install"
      rc=1
    fi
  done
  rm -rf "$d"
  return $rc
}
p_slot() {
  todo=
  for p in $(slot_paths); do
    v=$(as_user "$R$p" --version 2>/dev/null)
    [ "$v" = "slot $SLOT_VERSION" ] && continue
    drift "slot:$p" "is '${v:-missing}', config says slot $SLOT_VERSION"
    todo="$todo $p"
  done
  if fixing && [ -n "$todo" ]; then
    install_slot $todo && changed slot "slot $SLOT_VERSION installed at$todo" && slot_replaced=1
  fi
}

# Downloads the release archive of a pinned tool into directory $1, checks its SHA-256 and prints
# the directory that holds its binaries. Arguments: dir version sha256 base archive-format
# binaries... NODE_TEST=1: stub binaries, or nothing when $R/.state-tool-fetch-fails exists.
tool_fetch() {
  fd=$1 fv=$2 fs=$3 fu=$4
  fa=$(printf "$5" "$fv")
  shift 5
  case $fu in
    *%s*) fl=$(printf "$fu" "$fv")/$fa.tar.gz ;;
    *) fl=$fu/$fv/$fa.tar.gz ;;
  esac
  if fake_fetch; then
    [ -e "$R/.state-tool-fetch-fails" ] && return 0
    mkdir -p "$fd/$fa" || return 0
    for b in "$@"; do printf '#!/bin/sh\necho %s %s\n' "$b" "$fv" >"$fd/$fa/$b" && chmod 755 "$fd/$fa/$b"; done
    calls "fetch $fl"
  else
    (cd "$fd" && curl -fsSLO "$fl" &&
      echo "$fs  $fa.tar.gz" | sha256sum -c --strict - >/dev/null &&
      tar -xzf "$fa.tar.gz" --no-same-owner) || return 0
  fi
  chmod -R go+rX "$fd"
  chmod -R u-s,g-s "$fd"
  # An archive that holds its binaries at its top has no directory of its own name.
  if [ -d "$fd/$fa" ]; then echo "$fd/$fa"; else echo "$fd"; fi
}
# The first two words of the first line of a binary's `--version`, run as the build user. A
# first line that is only a version (task prints `3.54.0`) reads as `<binary> <version>`.
tool_version() {
  tv_l=$(as_user "$1" --version 2>/dev/null </dev/null | head -n 1 | cut -d' ' -f1,2)
  case $tv_l in [0-9]*) tv_l="${1##*/} $tv_l" ;; esac
  echo "$tv_l"
}
# The pinned tools (TOOL_PINS) in the store's cargo bin, the build user's ~/.cargo/bin: each
# binary answers `<binary> <version>`; apply installs the ones that do not and reads them back.
p_tools() {
  while read -r tn tv ts tu tf tb; do
    [ -n "$tn" ] || continue
    todo=
    for b in $tb; do
      p=$STORE/cargo/bin/$b
      v=$(tool_version "$R$p")
      [ "$v" = "$b $tv" ] && continue
      drift "tool:$tn" "$p is '${v:-missing}', want $b $tv"
      todo="$todo $b"
    done
    fixing && [ -n "$todo" ] || continue
    if [ -n "$T" ]; then td=$(mktemp -d); else td=$(mktemp -d /var/tmp/slot-tool.XXXXXX); fi || {
      failed "tool:$tn" "no temporary directory"
      continue
    }
    src=$(tool_fetch "$td" "$tv" "$ts" "$tu" "$tf" $tb </dev/null)
    if [ -z "$src" ]; then
      rm -rf "$td"
      failed "tool:$tn" "download or checksum of $tn $tv"
      continue
    fi
    done_ok=1
    for b in $todo; do
      p=$STORE/cargo/bin/$b
      if ! no_link "$p"; then
        failed "tool:$tn" "$p has a symbolic link in it; not installed"
        done_ok=
        continue
      fi
      as_user sh -c 'mkdir -p "$(dirname "$2")" && install -m 0755 "$1" "$2.slot-new" && mv "$2.slot-new" "$2"' sh "$src/$b" "$R$p" </dev/null
      v=$(tool_version "$R$p")
      if [ "$v" != "$b $tv" ]; then
        failed "tool:$tn" "$p is '${v:-missing}' after the install"
        done_ok=
      fi
    done
    rm -rf "$td"
    [ -n "$done_ok" ] && changed "tool:$tn" "$tn $tv installed:$todo"
  done <<EOF
$TOOL_PINS
EOF
}

# Rust: rustup for the build user in the store (~/.rustup and ~/.cargo link there), then each
# toolchain of RUST_TOOLCHAINS with RUST_COMPONENTS and RUST_TARGETS. Nothing is copied from the
# workstation. A toolchain is present when its rustc exists in the store's rustup home.
# Downloads rustup-init into directory $1, checks its SHA-256 and prints its path.
# NODE_TEST=1: a stub, or nothing when $R/.state-tool-fetch-fails exists.
rustup_fetch() {
  if fake_fetch; then
    [ -e "$R/.state-tool-fetch-fails" ] && return 0
    printf '#!/bin/sh\necho rustup-init\n' >"$1/rustup-init" || return 0
    calls "fetch $RUSTUP_URL"
  else
    (cd "$1" && curl -fsSLO "$RUSTUP_URL" &&
      echo "$RUSTUP_SHA256  rustup-init" | sha256sum -c --strict - >/dev/null) || return 0
  fi
  chmod 755 "$1" "$1/rustup-init"
  echo "$1/rustup-init"
}
# Runs a command as the build user with the store's rust homes, from a directory it can read.
as_rust_user() {
  (cd / && as_user env HOME="$H" RUSTUP_HOME="$STORE/rustup" CARGO_HOME="$STORE/cargo" \
    RUSTUP_INIT_SKIP_PATH_CHECK=yes PATH="$STORE/cargo/bin:/usr/local/bin:/usr/bin:/bin" "$@" </dev/null)
}
a_rustup_init() {
  if [ -n "$T" ]; then
    calls "rustup-init --no-modify-path --default-toolchain none --profile minimal -y"
    mkdir -p "$R$STORE/cargo/bin" && printf '#!/bin/sh\necho rustup\n' >"$R$STORE/cargo/bin/rustup" &&
      chmod 755 "$R$STORE/cargo/bin/rustup"
  else
    as_rust_user "$1" --no-modify-path --default-toolchain none --profile minimal -y
  fi
}
# The entries of list $2 (RUST_TARGETS or RUST_COMPONENTS) that apply to toolchain $1, one per
# line: each bare entry, and each entry `<toolchain>:<name>,<name>` that names this toolchain.
rust_entries() {
  for rt_e in $2; do
    case $rt_e in
      *:*) [ "${rt_e%%:*}" = "$1" ] && printf '%s\n' "${rt_e#*:}" | tr ',' '\n' ;;
      *) printf '%s\n' "$rt_e" ;;
    esac
  done | grep .
}
rust_targets() { rust_entries "$1" "$RUST_TARGETS"; }
rust_components() { rust_entries "$1" "$RUST_COMPONENTS"; }
rust_args() {
  for c in $(rust_components "$1"); do printf ' -c %s' "$c"; done
  for t in $(rust_targets "$1"); do printf ' -t %s' "$t"; done
}
rust_dir() {
  case $1 in
    *-$RUST_TRIPLE) echo "$STORE/rustup/toolchains/$1" ;;
    *) echo "$STORE/rustup/toolchains/$1-$RUST_TRIPLE" ;;
  esac
}
a_rustup_install() {
  if [ -n "$T" ]; then
    calls "rustup toolchain install $1 --profile minimal$2"
    [ -e "$R/.state-tool-fetch-fails" ] && return 1
    d=$R$(rust_dir "$1")
    mkdir -p "$d/bin" && printf '#!/bin/sh\necho "commit-hash: 0000000"\n' >"$d/bin/rustc" && chmod 755 "$d/bin/rustc"
    : >"$d/.components"
    : >"$d/.targets"
    for c in $(rust_components "$1"); do echo "$c-$RUST_TRIPLE" >>"$d/.components"; done
    echo "$RUST_TRIPLE" >>"$d/.targets"
    for t in $(rust_targets "$1"); do echo "$t" >>"$d/.targets"; done
  else
    as_rust_user "$STORE/cargo/bin/rustup" toolchain install "$1" --profile minimal $2
  fi
}
# The components or targets (kind $1: component or target) installed on toolchain $2, one per line.
q_rust_list() {
  if [ -n "$T" ]; then
    cat "$R$(rust_dir "$2")/.$1s" 2>/dev/null
  else
    as_rust_user "$STORE/cargo/bin/rustup" "$1" list --installed --toolchain "$2" 2>/dev/null
  fi
}
# Adds components or targets: kind, toolchain, names...
a_rustup_add() {
  ra_k=$1 ra_n=$2
  shift 2
  if [ -n "$T" ]; then
    calls "rustup $ra_k add --toolchain $ra_n $*"
    for ra_w in "$@"; do
      if [ "$ra_k" = component ]; then echo "$ra_w-$RUST_TRIPLE" >>"$R$(rust_dir "$ra_n")/.components"; else echo "$ra_w" >>"$R$(rust_dir "$ra_n")/.targets"; fi
    done
  else
    as_rust_user "$STORE/cargo/bin/rustup" "$ra_k" add --toolchain "$ra_n" "$@"
  fi
}
# A toolchain that is there has every RUST_COMPONENTS and RUST_TARGETS: check names each missing
# one, apply adds those and no other.
p_rust_extras() {
  for pe_k in component target; do
    if [ "$pe_k" = component ]; then pe_want=$(rust_components "$1"); else pe_want=$(rust_targets "$1"); fi
    [ -n "$pe_want" ] || continue
    pe_have=$(q_rust_list "$pe_k" "$1")
    pe_missing=
    for pe_w in $pe_want; do
      echo "$pe_have" | grep -qxF -e "$pe_w" -e "$pe_w-$RUST_TRIPLE" && continue
      drift "rust:$1" "$pe_k $pe_w missing"
      pe_missing="$pe_missing $pe_w"
    done
    fixing && [ -n "$pe_missing" ] && [ -n "$rustup_ok" ] || continue
    pe_l=$(mktemp)
    if a_rustup_add "$pe_k" "$1" $pe_missing >"$pe_l" 2>&1; then
      changed "rust:$1" "$pe_k added:$pe_missing"
    else
      tail -n 5 "$pe_l" >&2
      failed "rust:$1" "rustup $pe_k add$pe_missing"
    fi
    rm -f "$pe_l"
  done
}
# Removes components or targets: kind, toolchain, names...
a_rustup_remove() {
  ar_k=$1 ar_n=$2
  shift 2
  if [ -n "$T" ]; then
    calls "rustup $ar_k remove --toolchain $ar_n $*"
    ar_f=$R$(rust_dir "$ar_n")/.${ar_k}s
    for ar_w in "$@"; do
      if [ "$ar_k" = component ]; then ar_w=$ar_w-$RUST_TRIPLE; fi
      grep -vxF "$ar_w" "$ar_f" >"$ar_f.new"
      mv "$ar_f.new" "$ar_f"
    done
  else
    as_rust_user "$STORE/cargo/bin/rustup" "$ar_k" remove --toolchain "$ar_n" "$@"
  fi
}
# A target that a toolchain has, other than the host's, and that no RUST_TARGETS entry names for it
# is surplus; so is one of the components clippy, rustfmt, rust-src and llvm-tools (the ones the
# manifest line of toolchains.sh carries) that no RUST_COMPONENTS entry names for it. The node
# mirrors the workstation's per toolchain: with no entry at all, every non-host target is
# surplus. check names each surplus item and apply removes it.
p_rust_surplus() {
  for ps_k in target component; do
    if [ "$ps_k" = component ]; then ps_want=$(rust_components "$1"); else ps_want=$(rust_targets "$1"); fi
    ps_extra=
    for ps_h in $(q_rust_list "$ps_k" "$1"); do
      if [ "$ps_k" = component ]; then
        ps_h=${ps_h%-"$RUST_TRIPLE"}
        ps_h=${ps_h%-preview}
        case $ps_h in clippy | rustfmt | rust-src | llvm-tools) ;; *) continue ;; esac
      else
        [ "$ps_h" = "$RUST_TRIPLE" ] && continue
      fi
      echo "$ps_want" | grep -qxF "$ps_h" && continue
      drift "rust:$1" "$ps_k $ps_h is not in RUST_$(echo "$ps_k" | tr a-z A-Z)S"
      ps_extra="$ps_extra $ps_h"
    done
    fixing && [ -n "$ps_extra" ] && [ -n "$rustup_ok" ] || continue
    ps_l=$(mktemp)
    if a_rustup_remove "$ps_k" "$1" $ps_extra >"$ps_l" 2>&1; then
      changed "rust:$1" "$ps_k removed:$ps_extra"
    else
      tail -n 5 "$ps_l" >&2
      failed "rust:$1" "rustup $ps_k remove$ps_extra"
    fi
    rm -f "$ps_l"
  done
}
p_rust() {
  [ -n "$RUST_TOOLCHAINS" ] || return 0
  rb=$STORE/cargo/bin/rustup
  rustup_ok=1
  if [ ! -x "$R$rb" ]; then
    drift rustup missing
    rustup_ok=
    if fixing; then
      if [ -n "$T" ]; then rd=$(mktemp -d); else rd=$(mktemp -d /var/tmp/slot-rustup.XXXXXX); fi || {
        failed rustup "no temporary directory"
        return 0
      }
      rf=$(rustup_fetch "$rd")
      if [ -z "$rf" ]; then
        failed rustup "download or checksum of rustup-init $RUSTUP_VERSION"
      else
        rl=$(mktemp)
        if a_rustup_init "$rf" >"$rl" 2>&1 && [ -x "$R$rb" ]; then
          rustup_ok=1
          changed rustup "rustup $RUSTUP_VERSION installed in $STORE/rustup"
        else
          tail -n 5 "$rl" >&2
          failed rustup "rustup-init $RUSTUP_VERSION did not install rustup"
        fi
        rm -f "$rl"
      fi
      rm -rf "$rd"
    fi
  fi
  for n in $RUST_TOOLCHAINS; do
    ra=$(rust_args "$n")
    if [ -x "$R$(rust_dir "$n")/bin/rustc" ]; then
      p_rust_extras "$n"
      p_rust_surplus "$n"
      continue
    fi
    drift "rust:$n" missing
    fixing && [ -n "$rustup_ok" ] || continue
    rl=$(mktemp)
    if a_rustup_install "$n" "$ra" >"$rl" 2>&1 && [ -x "$R$(rust_dir "$n")/bin/rustc" ]; then
      changed "rust:$n" "toolchain $n installed"
    else
      tail -n 5 "$rl" >&2
      failed "rust:$n" "rustup toolchain install $n"
    fi
    rm -f "$rl"
  done
}

# The pinned archives (ARCHIVE_PINS): unpacked as root into /usr/local/<name>-<version>, each
# binary linked into /usr/local/bin. check reads each link and the version of the first binary.
# Downloads the archive into directory $1, checks its SHA-256, unpacks it into $1/tree and prints
# that path. Arguments: dir name version sha256 url strip binaries... NODE_TEST=1: stub binaries,
# or nothing when $R/.state-tool-fetch-fails exists.
# A binary word of an ARCHIVE_PINS line: its path in the unpacked tree, and the name it is linked
# as in /usr/local/bin (the last path component, or the word after a colon).
archive_src() { echo "${1%%:*}"; }
archive_link() {
  case $1 in
    *:*) echo "${1#*:}" ;;
    *) echo "${1##*/}" ;;
  esac
}
archive_fetch() {
  fd=$1 fn=$2 fv=$3 fs=$4 fu=$5 fk=$6
  shift 6
  if fake_fetch; then
    [ -e "$R/.state-tool-fetch-fails" ] && return 0
    for b in "$@"; do
      fm=755
      # A word with a link name stands for a script that a tarball ships without the execute bit.
      case $b in *:*) fm=644 ;; esac
      b=$(archive_src "$b")
      mkdir -p "$fd/tree/$(dirname "$b")" || return 0
      if [ "$fn" = go ]; then
        printf '#!/bin/sh\necho "go version go%s linux/amd64"\n' "$fv" >"$fd/tree/$b"
      else
        printf '#!/bin/sh\necho %s\n' "$fv" >"$fd/tree/$b"
      fi
      chmod "$fm" "$fd/tree/$b"
    done
    calls "fetch $fu"
  else
    (cd "$fd" && curl -fsSLo archive "$fu" && echo "$fs  archive" | sha256sum -c --strict - >/dev/null &&
      mkdir tree && tar -xf archive -C tree --strip-components="$fk" --no-same-owner) || return 0
  fi
  chmod -R go-w,u-s,g-s "$fd/tree"
  chmod -R a+rX "$fd/tree"
  # A script of an npm tarball that is not marked executable there (prettier's bin/prettier.cjs).
  # A symbolic link is left alone: chmod would follow it out of the tree.
  for xb in "$@"; do
    xp=$fd/tree/$(archive_src "$xb")
    [ -L "$xp" ] || chmod a+x "$xp"
  done
  echo "$fd/tree"
}
# The first line of the version of binary $2 of archive $1, run as the build user.
archive_version() {
  if [ "$1" = go ]; then as_user "$2" version; else as_user "$2" --version; fi 2>/dev/null </dev/null | head -n 1
}
p_archives() {
  while read -r an av as au ak ab; do
    [ -n "$an" ] || continue
    dir=/usr/local/$an-$av
    first=$(archive_src "${ab%% *}")
    todo=
    for b in $ab; do
      ln_=$(archive_link "$b")
      b=$(archive_src "$b")
      got=$(readlink "$R/usr/local/bin/$ln_" 2>/dev/null)
      [ "$got" = "$dir/$b" ] && continue
      drift "archive:$an" "/usr/local/bin/$ln_ is '${got:-missing}', want a link to $dir/$b"
      todo=1
    done
    wv=$av
    [ "$an" = go ] && wv=go$av
    v=$(archive_version "$an" "$R$dir/$first")
    vok=
    for w in $v; do [ "$w" = "$wv" ] && vok=1; done
    if [ -z "$vok" ]; then
      drift "archive:$an" "$dir/$first says '${v:-missing}', want $wv"
      todo=1
    fi
    fixing && [ -n "$todo" ] || continue
    if [ -n "$T" ]; then td=$(mktemp -d); else td=$(mktemp -d /var/tmp/slot-archive.XXXXXX); fi || {
      failed "archive:$an" "no temporary directory"
      continue
    }
    src=$(archive_fetch "$td" "$an" "$av" "$as" "$au" "$ak" $ab </dev/null)
    if [ -z "$src" ]; then
      rm -rf "$td"
      failed "archive:$an" "download or checksum of $an $av"
      continue
    fi
    mkdir -p "$R/usr/local/bin"
    rm -rf "$R$dir.slot-new"
    if ! { mv "$src" "$R$dir.slot-new" && rm -rf "$R$dir" && mv "$R$dir.slot-new" "$R$dir"; }; then
      rm -rf "$td"
      failed "archive:$an" "$dir not written"
      continue
    fi
    rm -rf "$td"
    for b in $ab; do ln -sfn "$dir/$(archive_src "$b")" "$R/usr/local/bin/$(archive_link "$b")"; done
    v=$(archive_version "$an" "$R$dir/$first")
    vok=
    for w in $v; do [ "$w" = "$wv" ] && vok=1; done
    if [ -n "$vok" ]; then
      changed "archive:$an" "$an $av installed in $dir, linked in /usr/local/bin"
    else
      failed "archive:$an" "$dir/$first says '${v:-missing}' after the install"
    fi
  done <<EOF
$ARCHIVE_PINS
EOF
}

p_keys() {
  k=$H/.ssh/authorized_keys
  if [ -d "$R$H/.ssh" ] && [ -f "$R$k" ] && gen_keys | cmp -s - "$R$k"; then
    :
  else
    drift keys "$k is not the build key alone$([ -n "$open" ] && echo ' plus the setup key')"
    if fixing && { [ -L "$R$H/.ssh" ] || [ -L "$R$k" ]; }; then
      failed keys "$H/.ssh or $k is a symbolic link; not written"
    elif fixing; then
      a_chattr -i "$k" 2>/dev/null
      a_chattr -i "$H/.ssh" 2>/dev/null
      mkdir -p "$R$H/.ssh"
      gen_keys >"$R$k.slot-new" && chmod 600 "$R$k.slot-new" && a_chown "$U:$U" "$R$k.slot-new" && mv "$R$k.slot-new" "$R$k"
      chmod 700 "$R$H/.ssh"
      a_chown "$U:$U" "$R$H/.ssh"
      [ -n "$T" ] || restorecon "$k" 2>/dev/null
      changed keys "$k written$([ -n "$open" ] && echo ' with the setup key')"
    fi
  fi
  for p in "$H/.ssh" "$k"; do
    q_immutable "$p" && continue
    drift keys "$p is not immutable"
    if fixing; then a_chattr +i "$p" && changed keys "$p immutable"; fi
  done
  # A connection that logged in with the setup key stays logged in after the key leaves
  # authorized_keys; closing the setup login ends it.
  [ -z "$open" ] && [ -n "$SETUP_KEY" ] || return 0
  sfp=$(echo "$SETUP_KEY" | ssh-keygen -lf - 2>/dev/null | awk '{print $2}')
  [ -n "$sfp" ] || return 0
  lf=$(mktemp) || return 1
  q_key_logins "$sfp" >"$lf" 2>/dev/null || :
  while read -r kind id leader; do
    case $kind in
      session)
        drift keys "a login with the setup key is still running: session $id (leader pid $leader)"
        if fixing; then a_end_session "$id" && changed keys "setup login session $id ended (leader pid $leader)"; fi
        ;;
      pid)
        drift keys "a login with the setup key is still connected: pid $id"
        if fixing; then a_kill "$id" && changed keys "setup login pid $id ended"; fi
        ;;
    esac
  done <"$lf"
  rm -f "$lf"
}

imds_open=
p_imds() {
  q_imds && return 0
  drift imds "the metadata service is open to other users than root"
  if fixing; then
    a_start slot-imds-root-only.service
    [ -n "$T" ] && [ ! -e "$R/.state-imds-stays-open" ] && : >"$S/imds"
    if q_imds; then
      changed imds "slot-imds-root-only.service started"
    else
      failed imds "slot-imds-root-only.service started and the metadata service is still open to other users than root; no key, rust or pinned tool is installed"
      imds_open=1
    fi
  fi
}

# Why the running process of $1 is not what its unit file and the config say; nothing when it is.
stale_reason() {
  sr_want=$(unit_defs "$1")
  sr_got=$(q_loaded "$1")
  if [ "$sr_want" != "$sr_got" ]; then
    echo "the loaded definition is not the unit file's"
    return 0
  fi
  sr_env=$(q_procenv "$1")
  if [ -n "$sr_env" ]; then
    sr_f=$(mktemp)
    echo "$sr_env" >"$sr_f"
    sr_miss=$(echo "$sr_want" | grep '^Environment=' | grep -vxF -f "$sr_f" | head -n 1)
    rm -f "$sr_f"
    if [ -n "$sr_miss" ]; then
      echo "the process started without ${sr_miss#Environment=}"
      return 0
    fi
  fi
  if [ "$1" = slot-node.service ]; then
    sr_v=$(q_running_version "$1")
    if [ -n "$sr_v" ] && [ "$sr_v" != "slot $SLOT_VERSION" ]; then
      echo "it runs '$sr_v', the config says slot $SLOT_VERSION"
      return 0
    fi
  fi
  return 0
}

p_services() {
  started=
  for u in slot-storage.service slot-imds-root-only.service slot-toolchains-manifest.timer sccache-node.service slot-node.service; do
    q_active "$u" && continue
    drift "unit:$u" "not active"
    if fixing; then a_start "$u" && changed "unit:$u" "started" && started="$started $u"; fi
  done
  # A running unit that is not what its file and the config say restarts: sccache-node at once,
  # slot-node when no job runs (it holds the build user's jobs).
  for u in sccache-node.service slot-node.service; do
    q_active "$u" || continue
    case " $started " in *" $u "*) continue ;; esac
    why=$(stale_reason "$u")
    if [ -z "$why" ]; then
      case " $units_changed " in *" $u "*) fixing && why="its unit file changed" ;; esac
      [ "$u" = slot-node.service ] && [ -n "$slot_replaced" ] && why="the slot binary was replaced"
    fi
    if [ -z "$why" ] && [ "$u" = slot-node.service ]; then
      want=$(serve_args)
      got=$(q_running_args "$u")
      [ "$got" = "$want" ] || why="it runs '$got', its unit says '$want'"
    fi
    [ -n "$why" ] || continue
    drift running "$u: $why"
    fixing || continue
    if [ "$u" = slot-node.service ]; then
      j=$(q_jobs)
      if [ "${j:-1}" != 0 ]; then
        echo "pending: running: slot-node restarts when no job runs ($j now): $why" >&2
        continue
      fi
    fi
    a_restart "$u" && changed running "$u restarted: $why"
  done
}

case $mode in
  check | apply)
    p_packages
    p_user
    p_sshd
    p_storage
    p_dirs
    p_binds
    p_fstab
    p_links
    p_files
    p_units
    p_imds
    p_slot
    [ -n "$imds_open" ] || p_rust
    [ -n "$imds_open" ] || p_tools
    p_archives
    [ -n "$imds_open" ] || p_keys
    p_services
    if [ "$mode" = check ]; then
      echo "$ndrift drift"
      [ "$ndrift" -eq 0 ] || exit 1
    else
      echo "$nchanged changed"
      [ "$nfailed" -eq 0 ] || exit 1
    fi
    ;;
  storage)
    p_storage
    p_dirs
    p_binds
    [ "$nfailed" -eq 0 ] || exit 1
    ;;
  *) usage ;;
esac
