# conf.sh: reads a node config file (KEY=value lines) into variables. Sourced by provision.sh and
# push.sh. Everything private about a node (user, home, keys, mirror roots, hosts, the deny list)
# lives in that file, outside the repository; this file only knows the keys.
#
# conf_read FILE REQUIRED_KEY... refuses, with exit status 2 and one line on stderr:
#   a line that is not KEY=value, a key it does not know, a required key that is missing or
#   empty, and a value of the wrong form.

conf_keys='STORE BUILD_USER BUILD_UID BUILD_HOME BUILD_KEY SETUP_KEY SLOT_VERSION MIRROR_DIRS
GO_PRIVATE TOKENS MIN_FREE_DISK SCCACHE_GB TMP_GB EXTRA_PACKAGES DISK_MODEL NODE_NAME
NODE_SSH PUSH_HOST NODE_HOST NODE_RUN_DIR RUST_TOOLCHAINS HOME_COPIES SKIP_BINS'

conf_fail() {
  echo "node config: $*" >&2
  return 2
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
  EXTRA_PACKAGES=${CONF_EXTRA_PACKAGES:-}
  DISK_MODEL=${CONF_DISK_MODEL:-Amazon EC2 NVMe Instance Storage}
  NODE_NAME=${CONF_NODE_NAME:-this node}
  NODE_SSH=${CONF_NODE_SSH:-}
  PUSH_HOST=${CONF_PUSH_HOST:-}
  NODE_HOST=${CONF_NODE_HOST:-}
  NODE_RUN_DIR=${CONF_NODE_RUN_DIR:-}
  RUST_TOOLCHAINS=${CONF_RUST_TOOLCHAINS:-}
  HOME_COPIES=${CONF_HOME_COPIES:-}
  SKIP_BINS=${CONF_SKIP_BINS:-}
  conf_check_forms "$cf"
}

conf_check_forms() {
  for k in BUILD_UID TOKENS MIN_FREE_DISK SCCACHE_GB TMP_GB; do
    eval "v=\${$k}"
    case $v in
      '') ;;
      *[!0-9]*)
        conf_fail "$1: $k must be a whole number, not '$v'"
        return 2
        ;;
    esac
  done
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
