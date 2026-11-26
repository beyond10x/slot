# conf.sh: reads a node config file (KEY=value lines) into variables. Sourced by provision.sh and
# push.sh. Everything private about a node (user, home, keys, mirror roots, hosts, the deny list)
# lives in that file, outside the repository; this file only knows the keys.
#
# conf_read FILE REQUIRED_KEY... refuses, with exit status 2 and one line on stderr:
#   a line that is not KEY=value, a key it does not know, a required key that is missing or
#   empty, and a value of the wrong form.

conf_keys='STORE BUILD_USER BUILD_UID BUILD_HOME BUILD_KEY SETUP_KEY SLOT_VERSION MIRROR_DIRS
GO_PRIVATE TOKENS MIN_FREE_DISK SCCACHE_GB TMP_GB DOCKER_GB EXTRA_PACKAGES DISK_MODEL NODE_NAME
NODE_SSH PUSH_HOST NODE_HOST NODE_RUN_DIR RUST_TOOLCHAINS RUST_COMPONENTS RUST_TARGETS
EXTRA_TOOLS
INSTANCE_PROFILE CPU_LIMIT CPU_PRESSURE DRAIN_MAX RUN_LIMIT RUN_LIMIT_MAX GO_ROOT
SLOT_TARGET CLANG_VERSION'

conf_fail() {
  echo "node config: $*" >&2
  return 2
}

# The seconds of the duration $1, in the forms the slot command line takes: whole seconds, or a
# whole number with s, m or h, at least 1 s and at most 9 digits. Nothing printed and status 1:
# no such duration.
conf_seconds() {
  case $1 in
    *[smh])
      cs_n=${1%?}
      cs_u=${1#"$cs_n"}
      ;;
    *)
      cs_n=$1
      cs_u=s
      ;;
  esac
  case $cs_n in '' | *[!0-9]* | ??????????*) return 1 ;; esac
  # Without leading zeros: the shell reads 010 as 8.
  cs_n=$(echo "$cs_n" | sed 's/^0*//')
  [ -n "$cs_n" ] || return 1
  case $cs_u in
    m) cs_n=$((cs_n * 60)) ;;
    h) cs_n=$((cs_n * 3600)) ;;
  esac
  echo "$cs_n"
}

conf_read() {
  cf=$1
  shift
  [ -r "$cf" ] || {
    conf_fail "cannot read $cf"
    return 2
  }
  ln=0
  while IFS= read -r line || [ -n "$line" ]; do
    ln=$((ln + 1))
    [ -n "$line" ] || continue
    case $line in
      [A-Z]*=*) ;;
      *)
        conf_fail "$cf:$ln: not KEY=value"
        return 2
        ;;
    esac
    k=${line%%=*}
    case " $(echo $conf_keys) " in
      *" $k "*) ;;
      *)
        conf_fail "$cf:$ln: unknown key $k"
        return 2
        ;;
    esac
    v=${line#*=}
    eval "CONF_$k=\$v"
  done <"$cf"
  for k in "$@"; do
    eval "v=\${CONF_$k:-}"
    [ -n "$v" ] || {
      conf_fail "$cf: required key $k is missing or empty"
      return 2
    }
  done
  STORE=${CONF_STORE:-/srv/slot}
  BUILD_USER=${CONF_BUILD_USER:-}
  BUILD_UID=${CONF_BUILD_UID:-}
  BUILD_HOME=${CONF_BUILD_HOME:-}
  BUILD_KEY=${CONF_BUILD_KEY:-}
  SETUP_KEY=${CONF_SETUP_KEY:-}
  SLOT_VERSION=${CONF_SLOT_VERSION:-}
  MIRROR_DIRS=${CONF_MIRROR_DIRS:-}
  GO_PRIVATE=${CONF_GO_PRIVATE:-}
  TOKENS=${CONF_TOKENS:-}
  MIN_FREE_DISK=${CONF_MIN_FREE_DISK:-50}
  SCCACHE_GB=${CONF_SCCACHE_GB:-200}
  TMP_GB=${CONF_TMP_GB:-400}
  DOCKER_GB=${CONF_DOCKER_GB:-10}
  EXTRA_PACKAGES=${CONF_EXTRA_PACKAGES:-}
  EXTRA_TOOLS=${CONF_EXTRA_TOOLS:-}
  DISK_MODEL=${CONF_DISK_MODEL:-Amazon EC2 NVMe Instance Storage}
  NODE_NAME=${CONF_NODE_NAME:-this node}
  NODE_SSH=${CONF_NODE_SSH:-}
  PUSH_HOST=${CONF_PUSH_HOST:-}
  NODE_HOST=${CONF_NODE_HOST:-}
  NODE_RUN_DIR=${CONF_NODE_RUN_DIR:-}
  RUST_TOOLCHAINS=${CONF_RUST_TOOLCHAINS:-}
  RUST_COMPONENTS=${CONF_RUST_COMPONENTS-clippy rustfmt}
  RUST_TARGETS=${CONF_RUST_TARGETS:-}
  INSTANCE_PROFILE=${CONF_INSTANCE_PROFILE:-}
  CPU_LIMIT=${CONF_CPU_LIMIT:-}
  CPU_PRESSURE=${CONF_CPU_PRESSURE:-}
  DRAIN_MAX=${CONF_DRAIN_MAX:-30m}
  RUN_LIMIT=${CONF_RUN_LIMIT:-10m}
  RUN_LIMIT_MAX=${CONF_RUN_LIMIT_MAX:-30m}
  GO_ROOT=${CONF_GO_ROOT:-}
  SLOT_TARGET=${CONF_SLOT_TARGET:-x86_64-unknown-linux-gnu}
  CLANG_VERSION=${CONF_CLANG_VERSION:-}
  conf_check_forms "$cf"
}

conf_check_forms() {
  for k in BUILD_UID TOKENS MIN_FREE_DISK SCCACHE_GB TMP_GB DOCKER_GB; do
    eval "v=\${$k}"
    case $v in
      '') ;;
      *[!0-9]*)
        conf_fail "$1: $k must be a whole number, not '$v'"
        return 2
        ;;
    esac
  done
  # A cap of 0 is over the cap at every check.
  case $DOCKER_GB in
    *[1-9]*) ;;
    *)
      conf_fail "$1: DOCKER_GB must be at least 1, not '$DOCKER_GB'"
      return 2
      ;;
  esac
  # The marks of the pool's CPU limit: digits with at most one decimal, or off. Empty is the
  # default, which provision.sh has (CPU_LIMIT follows the CPUs found). The daemon takes busy
  # CPUs from 1 and a pressure over 0 and at most 100, and keeps a pressure mark only beside a
  # CPU mark.
  for k in CPU_LIMIT CPU_PRESSURE; do
    eval "v=\${$k}"
    case $v in
      '' | off) continue ;;
      *[!0-9.]* | .* | *. | *.*.* | *.??*) v=\* ;;
    esac
    case $k in
      CPU_LIMIT)
        [ "$v" != \* ] && awk -v v="$v" 'BEGIN { exit !(v >= 1) }' || {
          conf_fail "$1: CPU_LIMIT must be busy CPUs from 1 with at most one decimal, or off, not '$CPU_LIMIT'"
          return 2
        }
        ;;
      *)
        [ "$v" != \* ] && awk -v v="$v" 'BEGIN { exit !(v > 0 && v <= 100) }' || {
          conf_fail "$1: CPU_PRESSURE must be a percent over 0 and at most 100 with at most one decimal, or off, not '$CPU_PRESSURE'"
          return 2
        }
        ;;
    esac
  done
  case $CPU_LIMIT:$CPU_PRESSURE in
    off: | off:off) ;;
    off:*)
      conf_fail "$1: CPU_PRESSURE must be off or empty when CPU_LIMIT is off, not '$CPU_PRESSURE'"
      return 2
      ;;
  esac
  # DRAIN_MAX, the bound of a drain (apply --drain): from 1m, since a shorter one ends the jobs
  # about as a restart does, to 24h, since the mark of a drain lives until the next boot and a
  # longer bound is none.
  cc_s=$(conf_seconds "$DRAIN_MAX") && [ "$cc_s" -ge 60 ] && [ "$cc_s" -le 86400 ] || {
    conf_fail "$1: DRAIN_MAX must be a duration from 1m to 24h (seconds, or a number with s, m or h), not '$DRAIN_MAX'"
    return 2
  }
  # The run-time limit of the pool as `slot run-limit` takes it: a duration or off, and the most
  # a job names, which the limit may not exceed (the daemon refuses that).
  cc_m=$(conf_seconds "$RUN_LIMIT_MAX") || {
    conf_fail "$1: RUN_LIMIT_MAX must be a duration of at least 1 s (seconds, or a number with s, m or h), not '$RUN_LIMIT_MAX'"
    return 2
  }
  if [ "$RUN_LIMIT" != off ]; then
    cc_s=$(conf_seconds "$RUN_LIMIT") || {
      conf_fail "$1: RUN_LIMIT must be a duration of at least 1 s (seconds, or a number with s, m or h), or off, not '$RUN_LIMIT'"
      return 2
    }
    [ "$cc_s" -le "$cc_m" ] || {
      conf_fail "$1: RUN_LIMIT '$RUN_LIMIT' is longer than RUN_LIMIT_MAX '$RUN_LIMIT_MAX', the most a job may name"
      return 2
    }
  fi
  case $INSTANCE_PROFILE in
    *[!A-Za-z0-9_+=,.@-]*)
      conf_fail "$1: INSTANCE_PROFILE must be the name of an instance profile, or none"
      return 2
      ;;
  esac
  for k in STORE BUILD_HOME; do
    eval "v=\${$k}"
    case $v in
      '') ;;
      /*[[:space:]]* | *[[:space:]]*)
        conf_fail "$1: $k must not contain spaces"
        return 2
        ;;
      /*) ;;
      *)
        conf_fail "$1: $k must be an absolute path"
        return 2
        ;;
    esac
  done
  # GO_ROOT, where the go archive goes in place of /usr/local/go-<version>: the path as `go env
  # GOROOT` prints it, so in its plain form. provision.sh replaces that directory whole, so it is
  # none that the provisioning writes elsewhere, none in one, and none that holds one.
  case $GO_ROOT in
    '') ;;
    /*[!A-Za-z0-9._/+-]* | /*//* | */ | /*/./* | /*/. | /*/../* | /*/.. | /./* | /../* | /. | /.. | [!/]*)
      conf_fail "$1: GO_ROOT must be an absolute path of letters, digits and the signs . _ + -, without a doubled slash, a slash at its end or a . or .. part, not '$GO_ROOT'"
      return 2
      ;;
    *)
      for v in "$STORE" "$BUILD_HOME" /usr/local/bin /usr/local/libexec /usr/bin /etc /run; do
        [ -n "$v" ] || continue
        case $GO_ROOT/ in "$v"/*) ;; *) case $v/ in "$GO_ROOT"/*) ;; *) continue ;; esac ;; esac
        conf_fail "$1: GO_ROOT '$GO_ROOT' is a directory the provisioning manages, lies in one or holds one ($v)"
        return 2
      done
      ;;
  esac
  case $SLOT_VERSION in
    '' | [0-9]*.[0-9]*.[0-9]*) ;;
    *)
      conf_fail "$1: SLOT_VERSION must be a release version such as 0.36.0"
      return 2
      ;;
  esac
  case $SLOT_VERSION in *[!0-9.]*)
    conf_fail "$1: SLOT_VERSION must be a release version such as 0.36.0"
    return 2
    ;;
  esac
  # Which archive of the release a node installs: the build against glibc, or the static one.
  case $SLOT_TARGET in
    x86_64-unknown-linux-gnu | x86_64-unknown-linux-musl) ;;
    *)
      conf_fail "$1: SLOT_TARGET must be x86_64-unknown-linux-gnu or x86_64-unknown-linux-musl, not '$SLOT_TARGET'"
      return 2
      ;;
  esac
  for k in BUILD_KEY SETUP_KEY; do
    eval "v=\${$k}"
    case $v in
      '' | ssh-ed25519\ * | ssh-rsa\ * | ecdsa-sha2-*\ *) ;;
      *)
        conf_fail "$1: $k must be one public key line (ssh-ed25519 ...)"
        return 2
        ;;
    esac
  done
  case $BUILD_USER in
    '' | [a-z_][a-z0-9_-]*) ;;
    *)
      conf_fail "$1: BUILD_USER '$BUILD_USER' is not a user name"
      return 2
      ;;
  esac
  case $BUILD_USER in *[!a-z0-9_-]*)
    conf_fail "$1: BUILD_USER '$BUILD_USER' is not a user name"
    return 2
    ;;
  esac
  # CLANG_VERSION: the major release of a clang package (clang22), which goes into package names
  # and paths; empty leaves the distribution's clang alone.
  case $CLANG_VERSION in
    '') ;;
    0* | *[!0-9]*)
      conf_fail "$1: CLANG_VERSION must be the major release of a clang package such as 22 (digits, no leading zero), or empty, not '$CLANG_VERSION'"
      return 2
      ;;
  esac
  # EXTRA_TOOLS: names of tools that provision.sh has a pin for; provision.sh, which knows the
  # pins, refuses a name it has none for.
  case $EXTRA_TOOLS in *[!a-z0-9\ -]*)
    conf_fail "$1: EXTRA_TOOLS must be names of lowercase letters, digits and dashes, separated by spaces"
    return 2
    ;;
  esac
  for k in RUST_TOOLCHAINS RUST_COMPONENTS RUST_TARGETS EXTRA_PACKAGES; do
    eval "v=\${$k}"
    case $k in
      EXTRA_PACKAGES) case $v in *[!A-Za-z0-9._+\ -]*) v=\* ;; esac ;;
      RUST_TARGETS | RUST_COMPONENTS) case $v in *[!A-Za-z0-9._:,\ -]* | *:-* | *,-*) v=\* ;; esac ;;
      *) case $v in *[!A-Za-z0-9._\ -]*) v=\* ;; esac ;;
    esac
    case " $v" in
      *\**)
        conf_fail "$1: $k must be names of letters, digits, dots, dashes and underscores (EXTRA_PACKAGES also +), separated by spaces"
        return 2
        ;;
      *" -"*)
        conf_fail "$1: $k must be names that do not begin with a dash"
        return 2
        ;;
    esac
    case $k in
      RUST_TARGETS | RUST_COMPONENTS)
        for w in $v; do
          case $k:$w in
            RUST_COMPONENTS:*-preview | RUST_COMPONENTS:*-preview,*)
              conf_fail "$1: $k entry '$w' uses the -preview spelling; write clippy, rustfmt or llvm-tools"
              return 2
              ;;
          esac
          case $w in
            *:*:*)
              conf_fail "$1: $k entry '$w' has more than one colon; the form is <toolchain>:<name>,<name>"
              return 2
              ;;
            :*)
              conf_fail "$1: $k entry '$w' has an empty toolchain before the colon"
              return 2
              ;;
            *:*)
              case " $RUST_TOOLCHAINS " in
                *" ${w%%:*} "*) ;;
                *)
                  conf_fail "$1: $k entry '$w' names '${w%%:*}', which is not a word of RUST_TOOLCHAINS"
                  return 2
                  ;;
              esac
              case ${w#*:} in
                '' | , | ,* | *, | *,,*)
                  conf_fail "$1: $k entry '$w' has an empty name after the colon"
                  return 2
                  ;;
              esac
              ;;
            *,*)
              conf_fail "$1: $k entry '$w' has a comma but no '<toolchain>:'; a bare entry is one name"
              return 2
              ;;
          esac
        done
        ;;
    esac
  done
  for d in $MIRROR_DIRS; do
    case $d in
      /* | *..*)
        conf_fail "$1: MIRROR_DIRS entry '$d' must be a path relative to BUILD_HOME"
        return 2
        ;;
    esac
  done
  return 0
}
