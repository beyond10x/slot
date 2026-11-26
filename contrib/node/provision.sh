#!/bin/sh
# provision.sh: brings a build node (Amazon Linux 2023 with NVMe instance storage) to the state a
# slot node needs, and reports where a node differs from it. Runs as root on the node.
#
#   provision.sh check CONF [--canary]
#                                     print one `drift: <part>: <what>` line per difference,
#                                     then `<n> drift`; exit 0 with 0 drift, 1 otherwise.
#                                     --canary also tries the node: a container against its
#                                     role, an aborted process, the load, a musl-gcc compile
#   provision.sh apply CONF [--open] [--drain]
#                                     change every part that drifts, print `changed: ...` per
#                                     change; --open also lets SETUP_KEY in (a setup login);
#                                     --drain starts a restart of slot-node that waits for its
#                                     jobs: the daemon takes no new job, ends when its jobs have
#                                     ended and starts again, DRAIN_MAX at most; no job is
#                                     signalled before that bound
#   provision.sh storage CONF         the boot step of slot-storage.service: instance store
#                                     array, its mount, the job tmp filesystem and the binds
#   provision.sh bundle CONF MODE [--open] [--drain] [--canary]
#                                     print one self-contained script (this script, conf.sh,
#                                     toolchains.sh and CONF) that runs MODE with the options
#                                     MODE takes, for a root channel that takes one script,
#                                     such as SSM Run Command
#   provision.sh pins                 print the cargo tools the node installs at a pinned
#                                     release, one `<binary> <version>` line each
#   provision.sh start-pre CONF       what systemd runs on the node before each start of
#                                     slot-node.service: the end of a drain
#   provision.sh drain-bound CONF     what the timer of a drain runs after DRAIN_MAX
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
canary=
drain=
usage() {
  sed -n '2,28p' "$here/provision.sh" | sed 's/^# \{0,1\}//' >&2
  exit 2
}
# An option a mode does not know is refused: a misspelled --canary would be a check without it.
# apply takes its two options in either order.
if [ "$mode" != bundle ]; then
  [ $# -le 4 ] || usage
  for opt in "${3:-}" "${4:-}"; do
    case $mode:$opt in
      *:) ;;
      apply:--open) open=1 ;;
      apply:--drain) drain=1 ;;
      check:--canary) canary=1 ;;
      *) usage ;;
    esac
  done
fi

# Tools the node installs at a pinned release into the store's cargo bin, the release its
# consumers' CI installs: a node job then runs the release its CI runs. One line each: name,
# version, SHA-256 of the release archive (.tar.gz), download directory (the version's directory
# under it, or with %s the version's directory itself), archive name (%s is the version), the
# binaries in it. The first line of each binary's `--version` starts with `<binary> <version>`.
# ess: the release the workstation has on PATH, a dispatcher that downloads the release a
# repository requires into the build user's ~/.cache/ess/toolchains (SHA-256 checked by ess).
# cargo-about: the release a consumer's CI installs, a static binary.
# geckodriver: the release on the forge runner image of the browser test lanes.
# sccache: the distribution has no package of it; the release the workstation runs.
TOOL_PINS='wasm-bindgen 0.2.130 9687f3bb551b9f66f37ed32a40d7913a00f7962db653054e56857bc306c3227f https://github.com/wasm-bindgen/wasm-bindgen/releases/download wasm-bindgen-%s-x86_64-unknown-linux-musl wasm-bindgen wasm-bindgen-test-runner wasm2es6js
geckodriver 0.37.1 e815130ea95983e162ae91843b48d3a3ce991735635fce83a647afde21e09f7e https://github.com/mozilla/geckodriver/releases/download/v%s geckodriver-v%s-linux64 geckodriver
sccache 0.18.0 45f1447fbe231e3037bde351ef70677dd212216c8d62ae7ca409fecc4d6acc89 https://github.com/mozilla/sccache/releases/download/v%s sccache-v%s-x86_64-unknown-linux-musl sccache
cargo-nextest 0.9.143 66786b9abe23920d022a182d1416b1bbc8130dd4872a9553d76985a1708dcd1e https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-%s cargo-nextest-%s-x86_64-unknown-linux-gnu cargo-nextest
task 3.54.0 680859dbb4d881a9c72d4d9a8f510825450849af8567deacd7302c01124416fb https://github.com/go-task/task/releases/download/v%s task_linux_amd64 task
ess 0.99.2 8b32cb02ac156929a873f4a1faf4c8c6d686d45565ce567ce2d360b9072dc42e https://github.com/beyond10x/ess/releases/download/%s ess-%s-x86_64-unknown-linux-gnu ess
cargo-about 0.9.1 c0e7dc6f5d74b0beec5c0053d39ab24514c717d19acd91886907a22457ea9e98 https://github.com/EmbarkStudios/cargo-about/releases/download/%s cargo-about-%s-x86_64-unknown-linux-musl cargo-about'
# Archives the node unpacks as root into /usr/local/<name>-<version> (the build user cannot
# replace them) and links into /usr/local/bin: the go and node releases the workstation runs, so
# that jobs resolve the same toolchain on both sides (a distribution package is older). One line
# each: name, version, SHA-256 of the archive, its URL, the leading path components to strip,
# the binaries to link (paths in the unpacked tree; the first one answers the version check:
# `go version` has a `go<version>` word, any other `--version` is the version; `path:name` links
# the binary at `path` as /usr/local/bin/<name>). Other releases: a consumer fetches its own
# (GOTOOLCHAIN=auto). typescript, prettier and esbuild are the npm tarballs of those releases
# (esbuild's is the native binary of its linux-x64 package), the releases the test lanes of the
# consumers read (`tsc`, `prettier` and `esbuild` on PATH). pnpm is the npm tarball of the release
# the workstation runs (its dist/ is bundled; it needs node 22.13 or later). yarn is the npm tarball
# of yarn 1.22.22, the release the workstation has in /usr/bin: one script, bin/yarn.js, linked as
# yarn and as yarnpkg, which needs node on PATH. yq is the Go one of mikefarah at the
# release the workstation runs; its tarball names the binary yq_linux_amd64. helm is the static
# binary of get.helm.sh at the release the workstation runs (its SHA-256 is the one that site
# publishes beside the archive); it has no `--version`, so its check reads
# `helm version --template {{.Version}}`.
ARCHIVE_PINS='go 1.27.1 63d339f0da5ab53635a56f2490a7984dfe12dfcff22ad749f63edaf590168445 https://go.dev/dl/go1.27.1.linux-amd64.tar.gz 1 bin/go bin/gofmt
node v22.23.3 1084aa36196bba4c3a5e69a1ee388a6e4ff729dad09445fbcd434b28fe3c24af https://nodejs.org/dist/v22.23.3/node-v22.23.3-linux-x64.tar.gz 1 bin/node bin/npm bin/npx bin/corepack
typescript 6.0.3 33cd0ee1beaa8c9e9d15a9da836c62ddea4c34a42d7c2d349dbc80d94165d22a https://registry.npmjs.org/typescript/-/typescript-6.0.3.tgz 1 bin/tsc bin/tsserver
prettier 3.8.1 5531dc6006ad06b642d5342438909f85dc53e87c50556753c908229b213fb4f4 https://registry.npmjs.org/prettier/-/prettier-3.8.1.tgz 1 bin/prettier.cjs:prettier
esbuild 0.28.2 9573bb2233aab0f9ea7647d5cca9726113cc1768de61d66b17267f4db84488f6 https://registry.npmjs.org/@esbuild/linux-x64/-/linux-x64-0.28.2.tgz 1 bin/esbuild
pnpm 11.9.0 2b567aa66026238078ac2e0a33bec3febd60e962987aac697456f3180819b287 https://registry.npmjs.org/pnpm/-/pnpm-11.9.0.tgz 1 bin/pnpm.mjs:pnpm bin/pnpx.mjs:pnpx
yarn 1.22.22 c17d3797fb9a9115bf375e31bfd30058cac6bc9c3b8807a3d8cb2094794b51ca https://registry.npmjs.org/yarn/-/yarn-1.22.22.tgz 1 bin/yarn.js:yarn bin/yarn.js:yarnpkg
yq v4.30.8 2e07c9b81699a6823dafc36a9c01aef5025179c069fedd42b1c6983545386771 https://github.com/mikefarah/yq/releases/download/v4.30.8/yq_linux_amd64.tar.gz 0 yq_linux_amd64:yq
helm v3.8.1 d643f48fe28eeb47ff68a1a7a26fc5142f348d02c8bc38d699674016716f61cd https://get.helm.sh/helm-v3.8.1-linux-amd64.tar.gz 1 helm'
# Archives in the form of ARCHIVE_PINS that a node installs only when its config names them
# (EXTRA_ARCHIVES), and the packages each loads (EXTRA_ARCHIVE_PACKAGES: the name, then the
# packages, installed with the base packages). firefox is the release archive of Mozilla for
# linux-x86_64, en-US, at the release the workstation runs; its SHA-256 is the line of that file in
# the release's SHA256SUMS on archive.mozilla.org. A consumer's browser tests start `firefox` by
# name, headless. The archive is a .tar.xz (xz unpacks it); the libraries its binaries need beside
# their own come from gtk3, alsa-lib, libX11-xcb and dbus-libs (and what gtk3 requires); the newest
# symbol versions they ask for are GLIBC_2.17 and GLIBCXX_3.4.19.
EXTRA_ARCHIVE_PINS='firefox 159.0 54536fcd120af484d4c56272a5a1cf4af57b9c713d1a3b1e775da192dd541fcd https://archive.mozilla.org/pub/firefox/releases/159.0/linux-x86_64/en-US/firefox-159.0.tar.xz 1 firefox'
EXTRA_ARCHIVE_PACKAGES='firefox gtk3 alsa-lib libX11-xcb dbus-libs xz'
RUSTUP_VERSION=1.29.3
RUSTUP_SHA256=b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55
RUSTUP_URL=https://static.rust-lang.org/rustup/archive/$RUSTUP_VERSION/x86_64-unknown-linux-gnu/rustup-init
RUST_TRIPLE=x86_64-unknown-linux-gnu
# musl, built on the node from its source tarball (p_musl): name, version, SHA-256, URL. The
# SHA-256 is the one of the tarball as downloaded from musl.libc.org on 2026-11-26, and the one
# that Arch Linux's package build file of musl 1.2.6-2 carries for the same tarball (`sha256sums`
# of its PKGBUILD, whose build checks the upstream signature): the two agree.
MUSL_PIN='musl 1.2.6 d585fd3b613c66151fc3249e8ed44f77020cb5e6c1e635a616d3f9f82460512a https://musl.libc.org/releases/musl-1.2.6.tar.gz'
# dash, built on the node from its source tarball (p_dash): name, version, SHA-256, URL. The
# release is the one of the forge's runner image (ubuntu-26.04, whose /bin/sh is the package dash
# 0.5.12-12ubuntu3), so that a shell test runs under the same shell on a node as on the forge.
# The SHA-256 is the one of upstream's release tarball as downloaded on 2026-11-26, the one of
# the `dash_0.5.12.orig.tar.gz` of Ubuntu's and of Debian's package pool, and the one that the
# `Checksums-Sha256` of Ubuntu's source package description (dash_0.5.12-12ubuntu3.dsc) carries for
# it: the four agree. Upstream serves the tarball over plain http only; the SHA-256 is what
# ties the download to the release.
DASH_PIN='dash 0.5.12 6a474ac46e8b0b32916c4c60df694c82058d3297d8b385b74508030ca4a8f28a http://gondor.apana.org.au/~herbert/dash/files/dash-0.5.12.tar.gz'
# Tools built on the node from a git revision, as the build user, into the store's cargo bin. One
# line each: name, version, repository URL, revision (the pin: a git object id; the release tag
# peels to it), package, rust toolchain (installed by the rust part), the binary (its `--version`
# starts with `<binary> <version>`). aep: the source build serves PATH (`aep` for a job); the
# release binary, which wants glibc 2.39 and only warns on a node's 2.34, is installed separately
# for runbook's driver cache path (HOME_PINS), where the driver demands the pin's digest.
MAKE_PIN='make 4.4.1 dd16fb1d67bfab79a72f5e8390735c49e3e8e70b4945a15ab1f81ddb78658fb3 https://ftp.gnu.org/gnu/make/make-4.4.1.tar.gz'
SOURCE_PINS='aep 0.80.0 https://github.com/beyond10x/aep af11af152de1732383a5edee0da723bb72d9acb4 aep-cli 1.99.1 aep'
# Trees in the build user's home that a job reads by path: the ess releases a repository pins by
# path (`~/.local/share/ess-versions/<version>/ess`) and the @types/node that ESS_TYPES_NODE names
# (`~/.local/share/ess-node/node_modules`, with its one dependency). One line each: name, version,
# SHA-256 of the archive, URL, leading path components to strip, directory under BUILD_HOME, the
# check (a binary of that directory that prints `<binary> <version>`, or `package.json`, whose
# `version` is read). The ess checksums are those of the release's SHA256SUMS; the npm ones were
# compared with the sha512 integrity of the workstation's package-lock.json. A line whose strip
# field is `zip` is a Playwright browser (the Chrome for Testing headless shell of a revision; the
# browsers.json of playwright-core 1.65.0 says 1254 is 158.0.8127.18, 1.62.1 says 1234 is
# 151.0.7922.34): unzipped without stripping, with the INSTALLATION_COMPLETE and
# DEPENDENCIES_VALIDATED markers, checked by the version `--version` prints. The libraries the
# binary needs come from EXTRA_PACKAGES, and `unzip` too. The aep line is the release binary that
# runbook's driver (`driver/aep-pin.json`) looks for at ~/.cache/sdc/aep/<version>/<target>/aep: its
# archive SHA-256 is the pin's `archive_sha256`, and the unpacked binary has the pin's
# `binary_sha256` (17cf105e...), which the driver compares. The release binary asks for
# GLIBC_2.39 (two weak symbols, pidfd_spawnp and pidfd_getpid); on a node with glibc 2.34 the loader
# prints one warning on stderr and `--version` still answers `aep 0.80.0`, which is all the driver
# reads. The groovy line is the Groovy 2.4.10 binary distribution of Maven Central (the SHA-256 is
# the one a consumer's CI script pins) at the path sdkman uses, which a consumer's tasks read as
# `${GROOVY_HOME:-~/.sdkman/candidates/groovy/2.4.10}` for `embeddable/groovy-all-2.4.10.jar` and
# `bin/groovyc`. Its strip field is `zip1`: a zip with one top directory, which is stripped. Its
# check is a path ending in `.jar`: the file exists and is no symbolic link (a jar prints no
# version; the marker file ties the tree to the archive).
HOME_PINS='ess 0.97.4 235ad6a993c3e81369e3b5439a4ef8d6e2b82df66ad8e4a22809e00557297938 https://github.com/beyond10x/ess/releases/download/0.97.4/ess-0.97.4-x86_64-unknown-linux-gnu.tar.gz 1 .local/share/ess-versions/0.97.4 ess
ess 0.98.0 c6c9a3a3d6956ce1a237f0340905dfadc431d31b1a15b104a6d2e64bb5acaa8e https://github.com/beyond10x/ess/releases/download/0.98.0/ess-0.98.0-x86_64-unknown-linux-gnu.tar.gz 1 .local/share/ess-versions/0.98.0 ess
ess 0.99.1 f69916ede161ef991fc96895ee2cc18f7e24cabac7e1de88f4bb85440fa6991d https://github.com/beyond10x/ess/releases/download/0.99.1/ess-0.99.1-x86_64-unknown-linux-gnu.tar.gz 1 .local/share/ess-versions/0.99.1 ess
ess 0.99.4 3902d0f8a6dffe5255bd0a9c8f56608fabf52faf14b97d90b6e7f7ed3da7032d https://github.com/beyond10x/ess/releases/download/0.99.4/ess-0.99.4-x86_64-unknown-linux-gnu.tar.gz 1 .local/share/ess-versions/0.99.4 ess
@types/node 26.3.0 69b0a537007774b0d5a3bea12736d3506b6ef451241fbca46213fd9e474b0a17 https://registry.npmjs.org/@types/node/-/node-26.3.0.tgz 1 .local/share/ess-node/node_modules/@types/node package.json
undici-types 8.3.0 07a721cb2cd0dd798c24757de34d14e8b640ff8fddef85d662e00b392562a1f2 https://registry.npmjs.org/undici-types/-/undici-types-8.3.0.tgz 1 .local/share/ess-node/node_modules/undici-types package.json
chrome-headless-shell 158.0.8127.18 18352d22cb0cdf520cbff2ad8c41391f0e02a72886cb53b0fde4bdfc4bd075d6 https://cdn.playwright.dev/builds/cft/158.0.8127.18/linux64/chrome-headless-shell-linux64.zip zip .cache/ms-playwright/chromium_headless_shell-1254 chrome-headless-shell-linux64/chrome-headless-shell
chrome-headless-shell 151.0.7922.34 3cfc2bd00d1bafcf8a68dc74c9c92bb7150ddc8d26ade948a776316e1cec4f14 https://cdn.playwright.dev/builds/cft/151.0.7922.34/linux64/chrome-headless-shell-linux64.zip zip .cache/ms-playwright/chromium_headless_shell-1234 chrome-headless-shell-linux64/chrome-headless-shell
aep 0.80.0 9277f3977397b540767497917bdb9666719249364901c466939343d1787d502a https://github.com/beyond10x/aep/releases/download/0.80.0/aep-0.80.0-x86_64-unknown-linux-gnu.tar.gz 1 .cache/sdc/aep/0.80.0/x86_64-unknown-linux-gnu aep
groovy 2.4.10 1c4dff3b6edf9a8ced3bca658ee1857cee90cfed1ee3474a2790045033c317a9 https://repo1.maven.org/maven2/org/codehaus/groovy/groovy-binary/2.4.10/groovy-binary-2.4.10.zip zip1 .sdkman/candidates/groovy/2.4.10 embeddable/groovy-all-2.4.10.jar'
pins() {
  echo "$TOOL_PINS" | while read -r _ pv _ _ _ pb; do for b in $pb; do echo "$b $pv"; done; done
  echo "$SOURCE_PINS" | while read -r _ pv _ _ _ _ pb; do for b in $pb; do echo "$b $pv"; done; done
}
if [ "$mode" = pins ]; then
  pins
  exit 0
fi
[ -n "$mode" ] && [ -n "$cf" ] || usage

BASE_PACKAGES='mdadm xfsprogs e2fsprogs rsync gcc gcc-c++ make clang lld pkgconf-pkg-config openssl-devel git tar gzip cmake protobuf-compiler perl iptables-nft chrony zstd python3 glibc-static bubblewrap socat libubsan unzip docker'
SSHD_LINES='MaxSessions 200
MaxStartups 100:30:200
ClientAliveInterval 15
ClientAliveCountMax 3'
UNITS='slot-storage.service slot-node.service sccache-node.service slot-imds-root-only.service slot-imds-containers.service slot-docker-prune.service slot-docker-prune.timer slot-toolchains-manifest.service slot-toolchains-manifest.timer'
ENABLED='slot-storage.service slot-node.service sccache-node.service slot-imds-root-only.service slot-imds-containers.service docker.service slot-docker-prune.timer slot-toolchains-manifest.timer'
LIBEXEC=/usr/local/libexec
# Tools that work only with a credential. No credential goes to a node, so none of them can work
# there: each is a script in the store's nodebin that says so (gen_no_tool).
NO_CREDENTIAL_TOOLS='glab gh'
# The node's kernel.core_pattern, in a file that sorts after systemd's 50-coredump.conf.
CORE_SYSCTL=/etc/sysctl.d/60-slot-core.conf
# Where the daemon of the pool answers which CPU limit it holds: the loopback address of `slot
# serve` when the unit names no --http.
CPU_LIMIT_URL=http://127.0.0.1:7411/api/cpu-limit
# The same daemon's answers about its run-time limit and about its jobs (the metrics also while
# it drains, when its socket is gone).
RUN_LIMIT_URL=http://127.0.0.1:7411/api/run-limit
METRICS_URL=http://127.0.0.1:7411/metrics
# A drain of slot-node.service (apply --drain): its marks, in a directory of root that is empty
# after a boot, and the name of the transient timer that is its bound.
DRAIN_DIR=/run/slot-node
DRAIN_UNIT=slot-node-drain
# The archives this node installs: ARCHIVE_PINS, then the lines of EXTRA_ARCHIVE_PINS that
# EXTRA_ARCHIVES names.
archive_pins() {
  echo "$ARCHIVE_PINS"
  echo "$EXTRA_ARCHIVE_PINS" | while read -r ap_n ap_r; do
    case " $EXTRA_ARCHIVES " in *" $ap_n "*) echo "$ap_n $ap_r" ;; esac
  done
}
# The packages of the extra archives that EXTRA_ARCHIVES names.
extra_archive_packages() {
  echo "$EXTRA_ARCHIVE_PACKAGES" | while read -r ep_n ep_p; do
    case " $EXTRA_ARCHIVES " in *" $ep_n "*) echo "$ep_p" ;; esac
  done
}
# EXTRA_ARCHIVES names archives of EXTRA_ARCHIVE_PINS only. conf.sh, which does not know the pins,
# checks the form of the names.
extra_archives_known() {
  ek_all=$(echo "$EXTRA_ARCHIVE_PINS" | awk '{ print $1 }' | tr '\n' ' ' | sed 's/ $//')
  for ek_n in $EXTRA_ARCHIVES; do
    case " $ek_all " in *" $ek_n "*) continue ;; esac
    conf_fail "$cf: EXTRA_ARCHIVES names '$ek_n', which is no archive of provision.sh ($ek_all)"
    return 2
  done
  return 0
}
# GO_ROOT may not be the directory of a pinned archive or of the make this script builds, nor lie
# in one: each of those is replaced whole at a release change. conf.sh, which does not know the
# pins, refuses the other directories of the provisioning.
go_root_pins() {
  [ -n "$GO_ROOT" ] || return 0
  for gp_d in $(archive_pins | awk '{ print "/usr/local/" $1 "-" $2 }') "/usr/local/make-$(echo "$MAKE_PIN" | cut -d' ' -f2)" "/usr/local/musl-$(echo "$MUSL_PIN" | cut -d' ' -f2)" "/usr/local/dash-$(echo "$DASH_PIN" | cut -d' ' -f2)"; do
    case $GO_ROOT/ in
      "$gp_d"/*)
        conf_fail "$cf: GO_ROOT '$GO_ROOT' is the directory of a pinned archive, or lies in one ($gp_d)"
        return 2
        ;;
    esac
  done
  return 0
}

if [ "$mode" = bundle ]; then
  conf_read "$cf" BUILD_USER BUILD_UID BUILD_HOME BUILD_KEY SLOT_VERSION || exit 2
  go_root_pins || exit 2
  extra_archives_known || exit 2
  m=${3:-}
  o=${4:-}
  [ $# -le 5 ] || usage
  case $m:$o:${5:-} in
    check:: | check:--canary: | apply:: | apply:--open: | apply:--drain: | apply:--open:--drain | apply:--drain:--open) ;;
    *) usage ;;
  esac
  o=$o${5:+ $5}
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
go_root_pins || exit 2
extra_archives_known || exit 2
H=$BUILD_HOME
U=$BUILD_USER
# The instance profile of this node: INSTANCE_PROFILE, else the one role.sh makes for NODE_NAME.
WANT_PROFILE=$INSTANCE_PROFILE
if [ -z "$WANT_PROFILE" ]; then
  case $NODE_NAME in
    *[!A-Za-z0-9_.-]*) ;;
    *) WANT_PROFILE=slot-node-$NODE_NAME ;;
  esac
fi

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
# The groups that list user $1 as a member, names on one line: what a process of a service gets
# beside the group of its unit. The user's own group is not among them.
q_groups() {
  if [ -n "$T" ]; then
    awk -F: -v u="$1" '$1 == u { print $2 }' "$S/groups" 2>/dev/null | tr '\n' ' '
  else
    getent group | awk -F: -v u="$1" '{ n = split($4, m, ","); for (i = 1; i <= n; i++) if (m[i] == u) print $1 }' | tr '\n' ' '
  fi
}
# The rules of slot-imds-containers.service are in the firewall.
q_imds_containers() {
  if [ -n "$T" ]; then
    [ -f "$S/imds-containers" ]
  else
    iptables -w -C DOCKER-USER -d 169.254.169.254/32 -j REJECT 2>/dev/null &&
      ip6tables -w -C DOCKER-USER -d fd00:ec2::254/128 -j REJECT 2>/dev/null
  fi
}
# The instance profile the metadata service names: its name, `none` when the instance has none,
# nothing when the service gives no answer. Asked as root, which the rule of
# slot-imds-root-only.service lets through.
#
# The four below read the metadata service; under NODE_TEST its answers are files: iam-info (the
# body) with iam-info-code, instance-role, imds-v1-code, not-ec2. No file is an instance without
# a profile whose service refuses a request without a token.
#
# (none) cannot be the name of a profile: a name has no parenthesis.
iam_info_profile() {
  case $1 in
    200) sed -n 's/.*"InstanceProfileArn"[^"]*"[^"]*\/\([^"/]*\)".*/\1/p' "$2" 2>/dev/null | head -n 1 ;;
    404) echo '(none)' ;;
  esac
}
q_instance_profile() {
  if [ -n "$T" ]; then
    if [ -f "$S/iam-info" ]; then
      iam_info_profile "$(cat "$S/iam-info-code" 2>/dev/null || echo 200)" "$S/iam-info"
    else
      iam_info_profile "$(cat "$S/iam-info-code" 2>/dev/null || echo 404)" /dev/null
    fi
  else
    qi_t=$(curl -sf -m 3 -X PUT -H 'X-aws-ec2-metadata-token-ttl-seconds: 60' http://169.254.169.254/latest/api/token 2>/dev/null) || return 0
    qi_f=$(mktemp) || return 0
    qi_c=$(curl -s -m 3 -o "$qi_f" -w '%{http_code}' -H "X-aws-ec2-metadata-token: $qi_t" http://169.254.169.254/latest/meta-data/iam/info 2>/dev/null)
    iam_info_profile "$qi_c" "$qi_f"
    rm -f "$qi_f"
  fi
}
# The role whose credentials the metadata service gives out; nothing when it names none.
q_instance_role() {
  if [ -n "$T" ]; then
    cat "$S/instance-role" 2>/dev/null
  else
    qr_t=$(curl -sf -m 3 -X PUT -H 'X-aws-ec2-metadata-token-ttl-seconds: 60' http://169.254.169.254/latest/api/token 2>/dev/null) || return 0
    curl -sf -m 3 -H "X-aws-ec2-metadata-token: $qr_t" http://169.254.169.254/latest/meta-data/iam/security-credentials/ 2>/dev/null | head -n 1
  fi
}
# What the metadata service does with a request that carries no token: `on` when it answers
# (IMDSv1), `off` when it refuses (401 with tokens required, 403 with the service off), nothing
# when neither is known (no answer, another status).
imds_v1_state() {
  case $1 in
    200) echo on ;;
    401 | 403) echo off ;;
  esac
}
q_imds_v1() {
  if [ -n "$T" ]; then
    imds_v1_state "$(cat "$S/imds-v1-code" 2>/dev/null || echo 401)"
  else
    imds_v1_state "$(curl -s -m 3 -o /dev/null -w '%{http_code}' http://169.254.169.254/latest/meta-data/ 2>/dev/null)"
  fi
}
# This machine is an EC2 instance.
q_ec2() {
  if [ -n "$T" ]; then
    [ ! -f "$S/not-ec2" ]
  else
    grep -q '^Amazon EC2' /sys/class/dmi/id/sys_vendor 2>/dev/null || grep -qi '^ec2' /sys/hypervisor/uuid 2>/dev/null
  fi
}
# The groups the main process of unit $1 started with, names on one line; nothing when the unit
# has no process.
q_proc_groups() {
  if [ -n "$T" ]; then
    cat "$S/procgroups/$1" 2>/dev/null
  else
    qm=$(systemctl show -p MainPID --value "$1" 2>/dev/null)
    [ -n "$qm" ] && [ "$qm" != 0 ] || return 0
    for qg in $(sed -n 's/^Groups:[[:space:]]*//p' "/proc/$qm/status" 2>/dev/null); do
      getent group "$qg" | cut -d: -f1
    done | tr '\n' ' '
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
# Which build of slot the file $1 is, as the target of its release archive (SLOT_TARGET): both
# builds answer the same `--version`. The build against glibc names a program interpreter (the
# loader); the static musl build has program headers and none. Read with readelf (binutils, which
# the base package gcc requires) as the build user. Nothing printed: no such file, no readelf, or
# no ELF file, and then nothing is compared. NODE_TEST=1: a stand-in that carries the musl target
# in its text is the musl build, any other file the gnu one.
q_slot_target() {
  if [ -n "$T" ]; then
    [ -f "$1" ] || return 0
    if grep -q 'x86_64-unknown-linux-musl' "$1" 2>/dev/null; then echo x86_64-unknown-linux-musl; else echo x86_64-unknown-linux-gnu; fi
  else
    qt=$(as_user env LC_ALL=C readelf -lW "$1" 2>/dev/null </dev/null) || return 0
    case $qt in
      *"Requesting program interpreter"*) echo x86_64-unknown-linux-gnu ;;
      *"Program Headers:"*) echo x86_64-unknown-linux-musl ;;
    esac
  fi
}
# The build of the slot binary the running unit executes (not the file at the path now).
# NODE_TEST=1: the file running-target/<unit>, which a_start writes.
q_running_target() {
  if [ -n "$T" ]; then
    cat "$S/running-target/$1" 2>/dev/null
  else
    qm=$(systemctl show -p MainPID --value "$1" 2>/dev/null)
    [ -n "$qm" ] && [ "$qm" != 0 ] && q_slot_target "/proc/$qm/exe"
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
# kernel.core_pattern as the kernel has it. NODE_TEST=1: the file core-pattern, and without it
# the pipe of a bare node.
q_core_pattern() {
  if [ -n "$T" ]; then
    cat "$S/core-pattern" 2>/dev/null || echo '|/usr/lib/systemd/systemd-coredump %P %u %g %s %t %c %h'
  else
    cat /proc/sys/kernel/core_pattern 2>/dev/null
  fi
}
# The soft core limit of the running main process of unit $1, as /proc has it (`0`, `unlimited`
# or a number of bytes): what a job of slot-node inherits. Nothing printed: no such process, or
# its limits cannot be read. NODE_TEST=1: the file core-limit/<unit>, which a_start writes, and
# without it 0.
q_core_limit() {
  if [ -n "$T" ]; then
    cat "$S/core-limit/$1" 2>/dev/null || echo 0
  else
    qm=$(systemctl show -p MainPID --value "$1" 2>/dev/null)
    case $qm in '' | 0 | *[!0-9]*) return 0 ;; esac
    awk '/^Max core file size/ { print $5 }' "/proc/$qm/limits" 2>/dev/null
  fi
}
# The soft core limit that the next main process of unit $1 gets: systemd's LimitCORESoft of the
# unit as it is loaded (`0`, `infinity` or a number of bytes). The step before a start of
# slot-node.service asks it: no process of the unit runs then. Nothing printed: no answer.
# NODE_TEST=1: the soft part of the unit file's LimitCORE, as a_start gives it to the process.
q_core_limit_next() {
  if [ -n "$T" ]; then
    qn=$(sed -n 's/^LimitCORE=\([^:]*\).*/\1/p' "$R/etc/systemd/system/$1" 2>/dev/null | head -n 1)
    echo "${qn:-unlimited}"
  else
    systemctl show -p LimitCORESoft --value "$1" 2>/dev/null
  fi
}
# The time until an aborted process is reaped, under the soft core limit 0 (what a job of
# slot-node runs with), whatever limit this run has: the quickest of 5 runs, in whole
# milliseconds. The shell's own line about the abort goes nowhere. NODE_TEST=1: the file
# abort-ms, and without it 2.
q_abort_ms() {
  if [ -n "$T" ]; then
    cat "$S/abort-ms" 2>/dev/null || echo 2
  else
    (
      ulimit -S -c 0
      qa_m=
      for qa_i in 1 2 3 4 5; do
        qa_0=$(date +%s%N)
        sh -c 'kill -ABRT $$'
        qa_1=$(date +%s%N)
        qa_d=$(((qa_1 - qa_0) / 1000000))
        if [ -z "$qa_m" ] || [ "$qa_d" -lt "$qa_m" ]; then qa_m=$qa_d; fi
      done
      echo "$qa_m"
    ) 2>/dev/null
  fi
}
# $1 is a plain decimal number: digits, at most one point, a digit on both sides of it.
is_decimal() {
  case $1 in '' | *[!0-9.]* | .* | *. | *.*.*) return 1 ;; esac
  return 0
}
# What the daemon of the pool says about its CPU limit: the JSON object of GET /api/cpu-limit on
# its loopback address (slot serve without --http), asked as the build user, 5 s and 64 KiB at
# most. The marks are the daemon's own, in a state directory the build user owns: root asks the
# daemon and opens no file there (a FIFO or a link in that place would hold or feed root).
# Nothing printed: no daemon answered. NODE_TEST=1: an answer made from the files cpu-limit and
# cpu-pressure (a_cpu_limit writes them), and none while slot-node.service is not active.
q_cpu_limit_answer() {
  if [ -n "$T" ]; then
    q_active slot-node.service || return 0
    qc_m=$(cat "$S/cpu-limit" 2>/dev/null)
    qc_p=$(cat "$S/cpu-pressure" 2>/dev/null)
    printf '{"cpu_busy_max":%s,"cpu_busy_floor":4,"cpus":%s,"busy":0.5,"held":0,"pool":0.2,"compile_cache":0.1,"outside":0.2,"cpu_pressure_max":%s,"pressure":0.7}\n' "${qc_m:-null}" "$(q_nproc)" "${qc_p:-null}"
  else
    as_user curl -s -m 5 --max-filesize 65536 "$CPU_LIMIT_URL" 2>/dev/null </dev/null | head -c 65536
  fi
}
# The same for the run-time limit: the JSON object of GET /api/run-limit, with `run_limit_s`
# (seconds, or null for no limit) and `run_limit_max_s` (the most a job names, seconds).
# NODE_TEST=1: an answer made from the files run-limit and run-limit-max (a_run_limit writes
# them; without them no limit and 3600, a daemon nobody set), and none while slot-node.service is
# not active.
q_run_limit_answer() {
  if [ -n "$T" ]; then
    q_active slot-node.service || return 0
    qr_l=$(cat "$S/run-limit" 2>/dev/null)
    qr_m=$(cat "$S/run-limit-max" 2>/dev/null)
    printf '{"run_limit_s":%s,"run_limit_max_s":%s}\n' "${qr_l:-null}" "${qr_m:-3600}"
  else
    as_user curl -s -m 5 --max-filesize 65536 "$RUN_LIMIT_URL" 2>/dev/null </dev/null | head -c 65536
  fi
}
# The main process of unit $1 as `<pid>@<start>`, the start in microseconds of the monotonic
# clock: a later process with the same PID has another name. Nothing printed: no main process.
# NODE_TEST=1: the file mainpid/<unit>, which a_start writes, while the unit is active.
q_main_id() {
  if [ -n "$T" ]; then
    q_active "$1" || return 0
    cat "$S/mainpid/$1" 2>/dev/null || echo 4000@0
  else
    systemctl show -p MainPID -p ExecMainStartTimestampMonotonic "$1" 2>/dev/null | awk -F= '
      $1 == "MainPID" { p = $2 }
      $1 == "ExecMainStartTimestampMonotonic" { s = $2 }
      END { if (p ~ /^[0-9]+$/ && p != 0) print p "@" s }'
  fi
}
# The time in seconds since the epoch (NODE_TEST=1: the file now, when there is one), and such a
# time as text in UTC.
q_now() {
  if [ -n "$T" ] && [ -e "$S/now" ]; then cat "$S/now"; else date +%s; fi
}
utc_of() { date -u -d "@$1" +%Y-%m-%dT%H:%M:%SZ; }
# The jobs of the pool that run or wait, from the daemon's /metrics, asked as the build user, 5 s
# at most: a daemon that drains has no socket for `slot status` (q_jobs) and still answers there.
# Nothing printed: no daemon answered with its metrics. NODE_TEST=1: the file jobs, and nothing
# when $R/.state-no-metrics exists.
q_jobs_left() {
  if [ -n "$T" ]; then
    [ -e "$R/.state-no-metrics" ] && return 0
    cat "$S/jobs" 2>/dev/null || echo 0
  else
    as_user curl -s -m 5 --max-filesize 1048576 "$METRICS_URL" 2>/dev/null </dev/null | awk '
      /^# TYPE slot_jobs_running gauge/ { ok = 1 }
      /^slot_jobs_(queued|running)\{/ { n += $NF }
      END { if (ok) print n + 0 }'
  fi
}
# The value of key $1 in the answer $2: `off` for null, the number as the daemon wrote it, `?`
# for anything else (a string, a negative number, a cut-off object). Nothing printed: no such key.
# Reads the answers about the CPU limit and about the run-time limit.
cpu_mark() {
  cm_v=$(printf '%s\n' "$2" | tr -d '\n' |
    sed -n "s/.*\"$1\"[[:space:]]*:[[:space:]]*\([^,}]*\).*/\1/p" | sed 's/[[:space:]]*$//')
  case $cm_v in
    '') ;;
    null) echo off ;;
    *) if is_decimal "$cm_v"; then echo "$cm_v"; else echo '?'; fi ;;
  esac
}
# The 5-minute load average: the second number of /proc/loadavg. NODE_TEST=1: the file loadavg in
# that format, and without it 1.00.
q_load5() {
  ql_f=/proc/loadavg
  if [ -n "$T" ]; then
    ql_f=$S/loadavg
    [ -e "$ql_f" ] || {
      echo 1.00
      return 0
    }
  fi
  cut -d' ' -f2 "$ql_f" 2>/dev/null
}

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
    # $R/.state-start-fails-<unit>: that unit does not start.
    [ -e "$R/.state-start-fails-$1" ] && return 1
    # Like systemd, which runs the unit file's ExecStartPre: the step before a start of
    # slot-node.service. What it prints goes to the file journal, except in the mode start-pre.
    if [ "$1" = slot-node.service ] && grep -q '^ExecStartPre=.* start-pre ' "$R/etc/systemd/system/$1" 2>/dev/null; then
      if [ "$mode" = start-pre ]; then gap_step; else gap_step >>"$S/journal"; fi
    fi
    st_touch active "$1"
    [ "$1" = slot-imds-containers.service ] && : >"$S/imds-containers"
    # A unit of a package (docker.service) has no file under /etc/systemd/system.
    [ -f "$R/etc/systemd/system/$1" ] || return 0
    mkdir -p "$S/running" "$S/loaded" "$S/procenv" "$S/running-version" "$S/procgroups" "$S/core-limit" "$S/mainpid"
    q_groups "$U" >"$S/procgroups/$1"
    # A new main process: the next number, as PID and as start time.
    as_n=$(($(cat "$S/mainpid-n" 2>/dev/null || echo 4000) + 1))
    echo "$as_n" >"$S/mainpid-n"
    echo "$as_n@$as_n" >"$S/mainpid/$1"
    # The process gets the soft core limit of its unit file; without one, systemd's default.
    as_l=$(sed -n 's/^LimitCORE=\([^:]*\).*/\1/p' "$R/etc/systemd/system/$1" | head -n 1)
    echo "${as_l:-unlimited}" >"$S/core-limit/$1"
    sed -n 's/^ExecStart=//p' "$R/etc/systemd/system/$1" >"$S/running/$1"
    unit_defs "$1" >"$S/loaded/$1"
    unit_defs "$1" | grep '^Environment=' >"$S/procenv/$1"
    [ "$1" = slot-node.service ] && "$R$STORE/bin/slot" --version >"$S/running-version/$1" 2>/dev/null
    [ "$1" = slot-node.service ] && mkdir -p "$S/running-target" && q_slot_target "$R$STORE/bin/slot" >"$S/running-target/$1"
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
# Loads the sysctl file $1. NODE_TEST=1: the pattern becomes `core`, unless
# $R/.state-core-stays exists.
a_sysctl_load() {
  if [ -n "$T" ]; then
    calls "sysctl -q -p $1"
    [ -e "$R/.state-core-stays" ] || { mkdir -p "$S" && echo core >"$S/core-pattern"; }
  else
    sysctl -q -p "$1"
  fi
}
# Sets the pool's CPU limit: `slot cpu-limit $1 --pressure $2`, or `slot cpu-limit off` (which
# takes the pressure mark away too), run as the build user against the running daemon. A daemon
# that started a moment ago may not answer yet: 5 tries, 1 s apart. NODE_TEST=1: the marks go to
# the files q_cpu_limit_answer reads, unless $R/.state-cpu-limit-stays exists.
a_cpu_limit() {
  if [ "$1" = off ]; then set -- off; else set -- "$1" --pressure "$2"; fi
  if [ -n "$T" ]; then
    calls "slot cpu-limit $*"
    [ -e "$R/.state-cpu-limit-stays" ] && return 0
    mkdir -p "$S" && rm -f "$S/cpu-limit" "$S/cpu-pressure"
    [ "$1" = off ] || echo "$1" >"$S/cpu-limit"
    [ "$1" = off ] || [ "$3" = off ] || echo "$3" >"$S/cpu-pressure"
    return 0
  fi
  for ac_i in 1 2 3 4 5; do
    as_user "$STORE/bin/slot" cpu-limit "$@" </dev/null && return 0
    [ "$ac_i" = 5 ] || sleep 1
  done
  return 1
}
# Sets the pool's run-time limit: `slot run-limit $1 --max $2` ($1 a duration or off, $2 the most
# a job names), as the build user against the running daemon, with the tries of a_cpu_limit. The
# daemon fixes the limit into a job when the job starts: the call ends no job that runs.
# NODE_TEST=1: the seconds go to the files q_run_limit_answer reads, unless
# $R/.state-run-limit-stays exists.
a_run_limit() {
  if [ -n "$T" ]; then
    calls "slot run-limit $1 --max $2"
    [ -e "$R/.state-run-limit-stays" ] && return 0
    mkdir -p "$S" && rm -f "$S/run-limit"
    [ "$1" = off ] || conf_seconds "$1" >"$S/run-limit"
    conf_seconds "$2" >"$S/run-limit-max"
    return 0
  fi
  for ar_i in 1 2 3 4 5; do
    as_user "$STORE/bin/slot" run-limit "$1" --max "$2" </dev/null && return 0
    [ "$ar_i" = 5 ] || sleep 1
  done
  return 1
}
# SIGTERM to the main process of unit $1 and to no other process of the unit: the daemon of the
# pool then drains (no new job, the jobs it has end, it exits). `systemctl stop` and `restart`
# signal every process of the unit (KillMode=control-group), the jobs too. NODE_TEST=1: the call
# is recorded, and fails when $R/.state-kill-fails exists.
a_signal_main() {
  if [ -n "$T" ]; then
    calls "systemctl kill --kill-whom=main --signal=TERM $1"
    [ ! -e "$R/.state-kill-fails" ]
  else
    systemctl kill --kill-whom=main --signal=TERM "$1"
  fi
}
# The bound of a drain: a transient timer that runs the mode drain-bound once, $1 seconds from
# now, with a clean environment. It belongs to the manager, so it outlives this run; its name is
# fixed, so there is one. A timer that an earlier drain left is stopped first. NODE_TEST=1: the
# file drain-timer stands for the timer; arming fails when $R/.state-drain-arm-fails exists.
a_drain_arm() {
  if [ -n "$T" ]; then
    calls "systemd-run --unit=$DRAIN_UNIT --on-active=$1 provision.sh drain-bound"
    [ -e "$R/.state-drain-arm-fails" ] && return 1
    mkdir -p "$S" && : >"$S/drain-timer"
  else
    systemctl stop "$DRAIN_UNIT.timer" 2>/dev/null
    systemd-run --quiet --collect --unit="$DRAIN_UNIT" --on-active="$1" --timer-property=AccuracySec=1s /usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin /bin/sh "$LIBEXEC/slot-node/provision.sh" drain-bound /etc/slot-node/node.conf
  fi
}
# Takes the bound away. A timer that already ran is gone, which is no error.
a_drain_disarm() {
  if [ -n "$T" ]; then
    [ -e "$S/drain-timer" ] || return 0
    calls "systemctl stop $DRAIN_UNIT.timer"
    rm -f "$S/drain-timer"
  else
    systemctl stop "$DRAIN_UNIT.timer" 2>/dev/null
    return 0
  fi
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
# slot-node.service runs one step as root before each start (ExecStartPre with `+`; `-`: a step
# that fails does not keep the pool down): the mode start-pre, which ends a drain (gap_step). Its
# environment is emptied first: the unit's PATH begins with a directory of the build user.
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
Environment=PATH=$STORE/nodebin:$H/.cargo/bin:/usr/local/bin:/usr/bin:/usr/sbin:$H/.bun/bin:$H/go/bin:$H/.local/bin:/bin GIT_TERMINAL_PROMPT=0 GIT_ASKPASS=$STORE/nodebin/no-credentials SSH_ASKPASS=$STORE/nodebin/no-credentials TMPDIR=$STORE/tmp$(go_env)
ExecStartPre=-+/usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin /bin/sh $LIBEXEC/slot-node/provision.sh start-pre /etc/slot-node/node.conf
ExecStart=$(serve_args)
ExecReload=/bin/kill -HUP \$MAINPID
KillMode=control-group
LimitNOFILE=65536
LimitCORE=0:infinity
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
LimitCORE=0:infinity
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
    slot-imds-containers.service)
      # A container's packets to the metadata service are forwarded, so the OUTPUT rule of
      # slot-imds-root-only.service does not see them. docker runs DOCKER-USER before its own
      # forward rules and keeps what is in it; docker.service requires this unit (the drop-in of
      # gen_docker_dropin), so no container starts without the rule. There is no ExecStop: a stop
      # would take the rule away under containers that still run. The IPv6 rule takes effect only
      # where dockerd manages ip6tables (it jumps to the IPv6 DOCKER-USER only then); without
      # that, docker gives a container no routed IPv6 either. p_docker_rule reads the rules back.
      cat <<'EOF'
[Unit]
Description=No container reaches the instance metadata service
DefaultDependencies=no
Before=network-pre.target docker.service
Wants=network-pre.target

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=-/usr/sbin/iptables -w -N DOCKER-USER
ExecStart=/bin/sh -c '/usr/sbin/iptables -w -C DOCKER-USER -d 169.254.169.254/32 -j REJECT 2>/dev/null || /usr/sbin/iptables -w -I DOCKER-USER 1 -d 169.254.169.254/32 -j REJECT'
ExecStart=-/usr/sbin/ip6tables -w -N DOCKER-USER
ExecStart=/bin/sh -c '/usr/sbin/ip6tables -w -C DOCKER-USER -d fd00:ec2::254/128 -j REJECT 2>/dev/null || /usr/sbin/ip6tables -w -I DOCKER-USER 1 -d fd00:ec2::254/128 -j REJECT'

[Install]
WantedBy=multi-user.target
EOF
      ;;
    slot-docker-prune.service)
      cat <<EOF
[Unit]
Description=Keep what docker stores within $DOCKER_GB GB
After=docker.service

[Service]
Type=oneshot
Nice=19
IOSchedulingClass=idle
TimeoutStartSec=300
ExecStart=/bin/sh $LIBEXEC/slot-node/docker-prune.sh $DOCKER_GB
EOF
      ;;
    slot-docker-prune.timer)
      cat <<'EOF'
[Unit]
Description=Docker prune check every 10 min

[Timer]
OnBootSec=300
OnUnitActiveSec=600
AccuracySec=30

[Install]
WantedBy=timers.target
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
gen_docker_dropin() {
  printf '[Unit]\nRequires=slot-imds-containers.service\nAfter=slot-imds-containers.service\n'
}
# docker-prune.sh GB: what the timer slot-docker-prune runs as root.
gen_docker_prune() {
  cat <<EOF
#!/bin/sh
SLOT_USER=$U
SLOT_UID=$BUILD_UID
SLOT_BIN=$STORE/bin/slot
EOF
  cat <<'EOF'
# docker-prune.sh GB: no container outlives the jobs, and what docker stores on the root disk
# (images, containers, volumes and the build cache, under /var/lib/docker and
# /var/lib/containerd) stays within GB.
#
# With no job in the pool (running or queued) every container is removed, running or not: a job
# that ended left it, and nothing else on the node starts one.
# Over the cap, at any time: containers that do not run and were created over an hour ago, and
# the build cache no build uses.
# Over the cap with no job in the pool: also every image and every volume no container uses.
# While the pool has a job its images and volumes stay, also one the job pulled, built or loaded
# and has not started yet. Then, and when slot does not answer, a line on stderr names what is
# held and the exit status is 1, as it is when a docker call fails.
cap=$(($1 * 1048576))
bad=0
used() { du -skx /var/lib/docker /var/lib/containerd 2>/dev/null | awk '{ s += $1 } END { print s + 0 }'; }
# Jobs in the pool, running and queued; nothing when slot does not answer.
pool() {
  runuser -u "$SLOT_USER" -- env XDG_RUNTIME_DIR="/run/user/$SLOT_UID" "$SLOT_BIN" status 2>/dev/null |
    head -1 | sed -n 's/.* \([0-9]*\) running, \([0-9]*\) queued.*/\1 \2/p' | awk '{ print $1 + $2 }'
}
dk() {
  docker "$@" >/dev/null && return 0
  echo "docker $* failed" >&2
  bad=1
}
# The containers are listed before the pool is asked: when the pool then has no job, none of the
# listed ones belongs to a job that runs.
left=$(docker ps -aq) || {
  echo "docker does not answer: nothing is pruned" >&2
  exit 1
}
j=$(pool)
if [ "$j" = 0 ] && [ -n "$left" ]; then
  docker rm -fv $left >/dev/null 2>&1
  now=" $(docker ps -aq | tr '\n' ' ') "
  n=0
  for c in $left; do
    case $now in
      *" $c "*)
        echo "container $c could not be removed" >&2
        bad=1
        ;;
      *) n=$((n + 1)) ;;
    esac
  done
  echo "removed containers left by jobs that ended: $n"
fi
u=$(used)
[ "$u" -le "$cap" ] && exit $bad
dk container prune -f --filter until=1h
dk builder prune -af
# Asked again: the calls above take time, and a job that came in between keeps its images.
if [ "$j" = 0 ] && [ "$(pool)" = 0 ]; then
  dk image prune -af
  dk volume prune -af
else
  j=${j:-unknown}
  [ "$j" = 0 ] && j='a new one'
fi
u2=$(used)
echo "docker held $((u / 1024)) MB, cap $1 GB: pruned to $((u2 / 1024)) MB"
[ "$u2" -le "$cap" ] && exit $bad
if [ "$j" = 0 ]; then
  echo "docker holds $((u2 / 1024)) MB with no job in the pool and everything unused removed, cap $1 GB" >&2
else
  echo "docker holds $((u2 / 1024)) MB, cap $1 GB: images and volumes stay while the pool has jobs (jobs: $j)" >&2
fi
exit 1
EOF
}
# role-canary.sh: run by `check --canary` and by hand.
gen_role_canary() {
  cat <<'EOF'
#!/bin/sh
# role-canary.sh: what a job can do with the node's role. A member of group docker can start a
# container as root on the host network, and the metadata service answers root there. This
# starts that container, takes the role's credentials from the metadata service inside it and
# asks Systems Manager for a parameter with them.
#
# Exit 0 only when Systems Manager refuses the call (AccessDenied), or when the instance has no
# role and so no credentials to give. Every other answer is exit 1 with one line on stderr: the
# call was allowed, the container did not run, or nothing could be tested.
#
# A refusal says that this one call is not allowed. What else the role allows is read by
# `role.sh check`, where the owner's identity is.
#
# Run as root or as a member of group docker. The image is the curl project's, named by digest;
# the first run pulls it, and the run ends after 5 minutes at the latest. The role's credentials
# reach curl through its standard input, not its arguments, and no line this prints carries
# them. The token of the metadata session (valid 60 s) is an argument of curl in the container.
IMAGE=docker.io/curlimages/curl:8.23.0@sha256:3fb7e13e5d179a845c97d78204f3f1064d1907037f26a3ebf91710571151fd93
out=$(
  timeout 300 docker run --rm -i --network host --user 0:0 --entrypoint /bin/sh "$IMAGE" -s 2>&1 <<'IN'
m=http://169.254.169.254/latest
t=$(curl -sf -m 5 -X PUT -H 'X-aws-ec2-metadata-token-ttl-seconds: 60' "$m/api/token") || {
  echo 'canary: no-token'
  exit 0
}
g() { curl -s -m 5 -H "X-aws-ec2-metadata-token: $t" "$m/meta-data/$1"; }
code=$(curl -s -m 5 -o /dev/null -w '%{http_code}' -H "X-aws-ec2-metadata-token: $t" "$m/meta-data/iam/security-credentials/")
if [ "$code" = 404 ]; then
  echo 'canary: no-role'
  exit 0
fi
role=$(g iam/security-credentials/ | head -n 1)
region=$(g placement/region)
c=$(g "iam/security-credentials/$role")
f() { printf '%s\n' "$c" | sed -n "s/.*\"$1\" *: *\"\([^\"]*\)\".*/\1/p" | head -n 1; }
if [ -z "$(f AccessKeyId)" ] || [ -z "$region" ]; then
  echo "canary: no-credentials $code"
  exit 0
fi
echo "canary: role $role"
printf 'user = "%s:%s"\nheader = "x-amz-security-token: %s"\n' "$(f AccessKeyId)" "$(f SecretAccessKey)" "$(f Token)" |
  curl -s -m 20 -K - --aws-sigv4 "aws:amz:$region:ssm" \
    -H 'Content-Type: application/x-amz-json-1.1' -H 'X-Amz-Target: AmazonSSM.GetParameter' \
    -d '{"Name":"slot-node-role-canary"}' "https://ssm.$region.amazonaws.com/" |
  sed -n 's/.*"__type" *: *"\([A-Za-z.#]*\)".*/canary: answer \1/p; t; s/.*"Parameter" *:.*/canary: answer Parameter/p'
echo
echo 'canary: end'
IN
) || {
  echo "role canary: the container did not run: $(echo "$out" | tail -n 2 | tr '\n' ' ')" >&2
  exit 1
}
role=$(echo "$out" | sed -n 's/^canary: role //p')
case $out in
  *'canary: no-role'*)
    echo "ok: the instance has no role: the metadata service has no credentials to give"
    exit 0
    ;;
  *'canary: no-token'*)
    echo "role canary: the metadata service gave the container no token: nothing was tested" >&2
    exit 1
    ;;
  *'canary: no-credentials'*)
    echo "role canary: the metadata service named no credentials or no region: nothing was tested" >&2
    exit 1
    ;;
  *'canary: answer AccessDeniedException'*)
    echo "ok: role $role: a root container on the host network got the role's credentials, and Systems Manager refused ssm:GetParameter (AccessDenied). One call: role.sh check reads the role"
    exit 0
    ;;
  *'canary: answer ParameterNotFound'* | *'canary: answer Parameter'*)
    echo "role canary: role $role may read parameters: Systems Manager allowed ssm:GetParameter to a container" >&2
    exit 1
    ;;
esac
echo "role canary: role ${role:-unknown}: no answer the canary knows: $(echo "$out" | grep '^canary: answer' | head -n 1 | cut -c1-200)" >&2
exit 1
EOF
}
gen_no_credentials() {
  cat <<EOF
#!/bin/sh
echo "slot: $NODE_NAME has no credentials (rule: no credential goes to the node). Clone or fetch on the workstation before the job. Use slot --here only for the step that reads a credential while it runs (a clone, a fetch, a push), never for a whole task: the rest of the task builds on the node. A step that needs a local service this node does not run goes in .slot.toml local = [...] of its repository." >&2
exit 255
EOF
}
# A tool of NO_CREDENTIAL_TOOLS: one line on stderr and exit 255, so a task that calls it ends
# there and not in what its next command makes of an empty answer.
gen_no_tool() {
  cat <<EOF
#!/bin/sh
echo "slot: $NODE_NAME has no $1: it works with a credential, and no credential goes to the node. Run the step that calls it on the workstation: slot --here <command>, or name the command in .slot.toml local = [...] of its repository." >&2
exit 255
EOF
}
gen_core_sysctl() { echo 'kernel.core_pattern=core'; }
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
echo "slot: $NODE_NAME has no credentials (rule: no credential goes to the node). Clone or fetch on the workstation before the job. Use slot --here only for the step that reads a credential while it runs (a clone, a fetch, a push), never for a whole task: the rest of the task builds on the node. A step that needs a local service this node does not run goes in .slot.toml local = [...] of its repository." >&2
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
  for p in $BASE_PACKAGES $EXTRA_PACKAGES $(extra_archive_packages); do q_pkg "$p" || missing="$missing $p"; done
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
  # The workstation's shims name the real binary by absolute path (/usr/bin/cargo, /usr/bin/go),
  # and the node runs that argv as sent: /usr/bin/<name> links to the node's own binary, never
  # over a real file.
  for n in cargo cargo-clippy cargo-fmt clippy-driver rustc rustdoc rustfmt rustup go gofmt; do
    p=/usr/bin/$n
    case $n in go | gofmt) t=/usr/local/bin/$n ;; *) t=$H/.cargo/bin/$n ;; esac
    if [ "$(readlink "$R$p" 2>/dev/null)" != "$t" ]; then
      drift "link:$p" "should point to $t"
      if fixing; then
        if [ -e "$R$p" ] && [ ! -L "$R$p" ]; then
          failed "link:$p" "is a real file; not replaced"
        else
          mkdir -p "$R/usr/bin" && rm -f "$R$p" && ln -s "$t" "$R$p" && changed "link:$p" "-> $t"
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
  for nt in $NO_CREDENTIAL_TOOLS; do
    want_file nodebin "$STORE/nodebin/$nt" 755 "$U:$U" gen_no_tool "$nt"
  done
  want_file sudoers /etc/sudoers.d/slot-node 440 root:root gen_sudoers
  want_file libexec "$LIBEXEC/slot-toolchains.sh" 755 root:root cat "$here/toolchains.sh"
  want_file libexec "$LIBEXEC/slot-node/provision.sh" 755 root:root cat "$here/provision.sh"
  want_file libexec "$LIBEXEC/slot-node/conf.sh" 644 root:root cat "$here/conf.sh"
  want_file libexec "$LIBEXEC/slot-node/docker-prune.sh" 755 root:root gen_docker_prune
  want_file libexec "$LIBEXEC/slot-node/role-canary.sh" 755 root:root gen_role_canary
  want_file libexec /etc/slot-node/node.conf 600 root:root cat "$cf"
}

units_changed=
slot_replaced=
draining=
p_units() {
  for u in $UNITS; do
    if ! want_file "unit:$u" "/etc/systemd/system/$u" 644 root:root gen_unit "$u"; then
      units_changed="$units_changed $u"
    fi
  done
  if ! want_file unit:docker.service.d /etc/systemd/system/docker.service.d/slot-imds.conf 644 root:root gen_docker_dropin; then
    units_changed="$units_changed docker.service.d"
  fi
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
# The archive is the one of SLOT_TARGET: the build against glibc (the default), or the static musl
# build. Both are checked against the line of their own name in the release's SHA256SUMS, so a
# release that has no such archive installs nothing.
slot_fetch() {
  a=slot-$SLOT_VERSION-$SLOT_TARGET
  if fake_fetch; then
    [ -e "$R/.state-slot-fetch-fails" ] && return 0
    if [ "$SLOT_TARGET" = x86_64-unknown-linux-gnu ]; then
      mkdir -p "$1/$a" && printf '#!/bin/sh\necho slot %s\n' "$SLOT_VERSION" >"$1/$a/slot" && chmod 755 "$1/$a/slot"
      calls "fetch slot $SLOT_VERSION"
    else
      mkdir -p "$1/$a" && printf '#!/bin/sh\n# %s\necho slot %s\n' "$SLOT_TARGET" "$SLOT_VERSION" >"$1/$a/slot" && chmod 755 "$1/$a/slot"
      calls "fetch slot $SLOT_VERSION $SLOT_TARGET"
    fi
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
      continue
    fi
    is_t=$(q_slot_target "$R$p")
    if [ -n "$is_t" ] && [ "$is_t" != "$SLOT_TARGET" ]; then
      failed slot "$p is the build for $is_t after the install, config says $SLOT_TARGET"
      rc=1
    fi
  done
  rm -rf "$d"
  return $rc
}
# A path is drift when it does not answer the release, or answers it and is the other build
# (SLOT_TARGET): a node changes between the gnu and the musl archive at one version.
p_slot() {
  todo=
  for p in $(slot_paths); do
    v=$(as_user "$R$p" --version 2>/dev/null)
    if [ "$v" = "slot $SLOT_VERSION" ]; then
      ps_t=$(q_slot_target "$R$p")
      if [ -z "$ps_t" ] || [ "$ps_t" = "$SLOT_TARGET" ]; then continue; fi
      drift "slot:$p" "is the build for $ps_t, config says $SLOT_TARGET"
    else
      drift "slot:$p" "is '${v:-missing}', config says slot $SLOT_VERSION"
    fi
    todo="$todo $p"
  done
  if fixing && [ -n "$todo" ]; then
    ps_n=
    [ "$SLOT_TARGET" = x86_64-unknown-linux-gnu ] || ps_n=" ($SLOT_TARGET)"
    install_slot $todo && changed slot "slot $SLOT_VERSION$ps_n installed at$todo" && slot_replaced=1
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
  tv_l=$(cd / && as_user "$1" --version 2>/dev/null </dev/null | head -n 1 | cut -d' ' -f1,2)
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
# The tools of SOURCE_PINS: built by `cargo install --locked --git <url> --rev <rev>` as the build
# user once the toolchain of the pin exists (the rust part runs first), read back with `--version`.
# --force: a binary of another release or one cargo did not install is replaced. TMPDIR: the
# store's tmp, not /tmp (a tmpfs, too small for the build). The build ends after 25 minutes.
a_cargo_install() {
  ai_v=$2
  ai_b=$7
  set -- "+$6" install --locked --force --git "$3" --rev "$4" "$5" --root "$STORE/cargo"
  if [ -n "$T" ]; then
    calls "cargo $*"
    [ -e "$R/.state-tool-fetch-fails" ] && return 1
    mkdir -p "$R$STORE/cargo/bin" && printf '#!/bin/sh\necho %s %s\n' "$ai_b" "$ai_v" >"$R$STORE/cargo/bin/$ai_b" &&
      chmod 755 "$R$STORE/cargo/bin/$ai_b"
  else
    as_rust_user env TMPDIR="$STORE/tmp" timeout 1500 "$STORE/cargo/bin/cargo" "$@"
  fi
}
p_source_tools() {
  while read -r sn sv su sr sp st sb; do
    [ -n "$sn" ] || continue
    p=$STORE/cargo/bin/$sb
    v=$(tool_version "$R$p")
    [ "$v" = "$sb $sv" ] && continue
    drift "tool:$sn" "$p is '${v:-missing}', want $sb $sv (built from $su at $sr)"
    fixing || continue
    if [ ! -x "$R$(rust_dir "$st")/bin/rustc" ]; then
      failed "tool:$sn" "rust toolchain $st is not installed; $sn $sv not built"
      continue
    fi
    if ! no_link "$p"; then
      failed "tool:$sn" "$p has a symbolic link in it; not built"
      continue
    fi
    if [ -n "$T" ]; then sl=$(mktemp); else sl=$(mktemp /var/tmp/slot-src.XXXXXX); fi || {
      failed "tool:$sn" "no temporary file"
      continue
    }
    if ! a_cargo_install "$sn" "$sv" "$su" "$sr" "$sp" "$st" "$sb" >"$sl" 2>&1; then
      failed "tool:$sn" "cargo install of $sn $sv failed: $({ grep -m 3 '^error' "$sl"; tail -n 2 "$sl"; } | tr '\n' ' ')"
      rm -f "$sl"
      [ -n "$T" ] || rm -rf "$R$STORE"/tmp/cargo-install* 2>/dev/null
      continue
    fi
    rm -f "$sl"
    v=$(tool_version "$R$p")
    if [ "$v" = "$sb $sv" ]; then
      changed "tool:$sn" "$sn $sv built from $sr: $sb"
    else
      failed "tool:$sn" "$p is '${v:-missing}' after the build"
    fi
  done <<EOF
$SOURCE_PINS
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
  p_rust_default
}

# The default toolchain is `stable` when RUST_TOOLCHAINS has it, like the workstation's: a job that
# names no toolchain (`cargo` with no `+<toolchain>` and no rust-toolchain file) runs it. Without
# this rustup makes the first toolchain it installs the default (1.85.0 on a node). check reads
# `default_toolchain` of the rustup home's settings.toml, apply runs `rustup default stable`.
a_rustup_default() {
  if [ -n "$T" ]; then
    calls "rustup default $1"
    [ -e "$R/.state-default-fails" ] && return 1
    printf 'version = "12"\ndefault_toolchain = "%s"\nprofile = "minimal"\n' "$(basename "$(rust_dir "$1")")" >"$R$STORE/rustup/settings.toml"
  else
    as_rust_user "$STORE/cargo/bin/rustup" default "$1"
  fi
}
p_rust_default() {
  case " $RUST_TOOLCHAINS " in *" stable "*) ;; *) return 0 ;; esac
  [ -x "$R$(rust_dir stable)/bin/rustc" ] || return 0
  want=$(basename "$(rust_dir stable)")
  got=$(sed -n "s/^[[:space:]]*default_toolchain[[:space:]]*=[[:space:]]*[\"']\\([^\"']*\\)[\"'].*/\\1/p" "$R$STORE/rustup/settings.toml" 2>/dev/null | head -n 1 | tr -cd 'A-Za-z0-9._-')
  [ "$got" = "$want" ] && return 0
  drift "rust:default" "the default toolchain is '${got:-none}', want $want"
  fixing && [ -n "$rustup_ok" ] || return 0
  rl=$(mktemp)
  if a_rustup_default stable >"$rl" 2>&1; then
    changed "rust:default" "rustup default stable ($want)"
  else
    tail -n 5 "$rl" >&2
    failed "rust:default" "rustup default stable"
  fi
  rm -f "$rl"
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
        # Like go: `env GOROOT` answers the directory the binary stands in.
        printf '#!/bin/sh\n[ "$*" != "env GOROOT" ] || { dirname "$(dirname "$0")"; exit 0; }\necho "go version go%s linux/amd64"\n' "$fv" >"$fd/tree/$b"
      elif [ "$fn" = helm ]; then
        # Like helm: no `--version`, the version without a newline for `version --template`.
        printf '#!/bin/sh\n[ "$*" = "version --template {{.Version}}" ] || exit 1\nprintf %%s "%s"\n' "$fv" >"$fd/tree/$b"
      elif [ "$fn" = yarn ]; then
        # Like yarn 1 under a `yarn-path` of a .yarnrc: its own release only with
        # YARN_IGNORE_PATH=1, else what that path prints.
        printf '#!/bin/sh\n[ "${YARN_IGNORE_PATH:-}" = 1 ] || { echo 9.9.9-from-yarn-path; exit 0; }\necho %s\n' "$fv" >"$fd/tree/$b"
      elif [ "$fn" = firefox ]; then
        # Like firefox: `--version` prints `Mozilla Firefox <release>`; a start prints the line of
        # WebDriver BiDi on standard error and runs until it is ended.
        printf '#!/bin/sh\n[ "$1" != --version ] || { echo "Mozilla Firefox %s"; exit 0; }\necho "WebDriver BiDi listening on ws://127.0.0.1:9222" >&2\nexec sleep 30\n' "$fv" >"$fd/tree/$b"
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
# The first line of the version of binary $2 of archive $1, run as the build user. yarn 1 runs
# the script a `yarn-path` of the build user's .yarnrc names and prints that one's version:
# YARN_IGNORE_PATH=1 makes it answer for itself. firefox reads MOZ_HEADLESS=1 and opens no window.
archive_version() {
  case $1 in
    go) as_user "$2" version ;;
    helm) as_user "$2" version --template '{{.Version}}' ;;
    yarn) as_user env YARN_IGNORE_PATH=1 "$2" --version ;;
    firefox) as_user env MOZ_HEADLESS=1 "$2" --version ;;
    *) as_user "$2" --version ;;
  esac 2>/dev/null </dev/null | head -n 1
}
# GO_ROOT: the directory of the go archive in place of /usr/local/go-<version>, for a node whose
# `go env GOROOT` must answer the path another machine's go answers. go takes its GOROOT from the
# real path of its binary, so a link at that path to the versioned directory would still answer
# the versioned one: the archive is unpacked there as a real directory. The path is one that
# others may own (a distribution's go lives in /usr/lib/go), so the directory carries a mark,
# the file .slot-archive with `go <version> <sha256 of the archive>`, and a path that exists
# without it, that is a symbolic link or that a package owns is not written to.
#
# What `go env GOROOT` of the go at $1 answers to the build user. NODE_TEST=1: the file goroot
# when there is one, else the stand-in's answer without the test root.
q_goroot() {
  if [ -n "$T" ]; then
    if [ -e "$S/goroot" ]; then
      cat "$S/goroot"
      return 0
    fi
    qg_v=$(as_user "$R$1" env GOROOT 2>/dev/null </dev/null | head -n 1)
    echo "${qg_v#"$R"}"
  else
    as_user "$1" env GOROOT 2>/dev/null </dev/null | head -n 1
  fi
}
# The package that owns path $1; nothing printed when none does. NODE_TEST=1: the file
# pkg-owner/<path with _ for />.
q_pkg_owner() {
  if [ -n "$T" ]; then
    cat "$S/pkg-owner/$(key "$1")" 2>/dev/null
  else
    qo_v=$(rpm -qf "$1" 2>/dev/null) && echo "$qo_v" | head -n 1
  fi
}
# Why GO_ROOT ($1) is not this script's to write; nothing printed when it is, or is not there.
go_root_foreign() {
  [ -e "$R$1" ] || [ -L "$R$1" ] || return 0
  if [ -L "$R$1" ]; then
    echo "is a symbolic link, owner $(q_owner "$1")"
    return 0
  fi
  gf_p=$(q_pkg_owner "$1")
  if [ -n "$gf_p" ]; then
    echo "belongs to the package $gf_p"
    return 0
  fi
  if [ -f "$R$1/.slot-archive" ] && [ ! -L "$R$1/.slot-archive" ]; then
    case $(head -n 1 "$R$1/.slot-archive") in "go "*) return 0 ;; esac
  fi
  echo "exists without the mark of provision.sh ($1/.slot-archive), owner $(q_owner "$1")"
}
# The /usr/local/go-<version> ($1) that go was in before GO_ROOT ($2), once go works from $2:
# removed when the pool counts no job, since a go build that runs reads its GOROOT. check names
# it with a pending line, which is no drift. A directory a link of /usr/local/bin still names
# stays (the link is drift).
go_old_dir() {
  [ -d "$R$1" ] && [ ! -L "$R$1" ] || return 0
  for go_b in go gofmt; do
    case $(readlink "$R/usr/local/bin/$go_b" 2>/dev/null) in "$1"/*) return 0 ;; esac
  done
  if ! fixing; then
    [ "$mode" != check ] || echo "pending: archive:go: $1 is removed when no job runs"
    return 0
  fi
  go_j=0
  if q_active slot-node.service; then go_j=$(q_jobs); fi
  if [ "${go_j:-1}" != 0 ]; then
    echo "pending: archive:go: $1 is removed when no job runs" >&2
    return 0
  fi
  if rm -rf "$R$1"; then
    changed archive:go "$1 removed: go is in $2"
  else
    failed archive:go "$1 not removed"
  fi
}
p_archives() {
  while read -r an av as au ak ab; do
    [ -n "$an" ] || continue
    dir=/usr/local/$an-$av
    old=
    if [ "$an" = go ] && [ -n "$GO_ROOT" ]; then
      old=$dir
      dir=$GO_ROOT
      aw=$(go_root_foreign "$dir")
      if [ -n "$aw" ]; then
        drift "archive:$an" "GO_ROOT $dir $aw"
        if fixing; then failed "archive:$an" "GO_ROOT $dir $aw: nothing was changed"; fi
        continue
      fi
    fi
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
    elif [ -n "$old" ]; then
      gr=$(q_goroot "$dir/$first")
      if [ "$gr" != "$dir" ]; then
        drift "archive:$an" "go env GOROOT of $dir/$first answers '${gr:-nothing}', want $dir"
        todo=1
      fi
    fi
    if [ -z "$todo" ]; then
      [ -z "$old" ] || go_old_dir "$old" "$dir" </dev/null
      continue
    fi
    fixing || continue
    if [ -n "$old" ] && ! no_link "$dir"; then
      failed "archive:$an" "GO_ROOT $dir has a symbolic link in its path: not written"
      continue
    fi
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
    # The mark goes into the tree before the tree goes to its place.
    if [ -n "$old" ]; then
      mkdir -p "$(dirname "$R$dir")"
      echo "$an $av $as" >"$src/.slot-archive" && chmod 644 "$src/.slot-archive"
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
    if [ -z "$vok" ]; then
      failed "archive:$an" "$dir/$first says '${v:-missing}' after the install"
      continue
    fi
    if [ -n "$old" ]; then
      gr=$(q_goroot "$dir/$first")
      if [ "$gr" != "$dir" ]; then
        failed "archive:$an" "go env GOROOT of $dir/$first answers '${gr:-nothing}' after the install, want $dir"
        continue
      fi
    fi
    changed "archive:$an" "$an $av installed in $dir, linked in /usr/local/bin"
    [ -z "$old" ] || go_old_dir "$old" "$dir" </dev/null
  done <<EOF
$(archive_pins)
EOF
}

# source_unpack DIR SHA256 URL: the source tarball of a pin that is built on the node (make,
# musl). Downloads it into DIR (root's temporary directory, made readable for the build user),
# checks its SHA-256 before anything of it is unpacked, and unpacks it as DIR/src, owned by the
# build user, who builds there. Status 1 when one of the steps failed.
source_unpack() {
  chmod 755 "$1"
  (cd "$1" && curl -fsSLo src.tar.gz "$3" && echo "$2  src.tar.gz" | sha256sum -c --strict - >/dev/null &&
    mkdir src && tar -xf src.tar.gz -C src --strip-components=1 --no-same-owner &&
    chown -R "$U:$U" src)
}
# GNU make built from its source tarball (MAKE_PIN): the node's make is 4.3, which cannot parse the
# `--jobserver-auth=fifo:` form of the pool's jobserver (make 4.4 added it), so every cmake or make
# build of a job failed with "invalid --jobserver-auth string". The tarball is built by the build
# user in a temporary directory; root installs the one binary into /usr/local/make-<version>/bin and
# links it as make and gmake in /usr/local/bin, which the job PATH has before /usr/bin.
# Prints the path of the built make; nothing when the download, the checksum or the build failed.
make_build() {
  mb_d=$1
  if fake_fetch; then
    [ -e "$R/.state-tool-fetch-fails" ] && return 0
    calls "fetch $5"
    printf '#!/bin/sh\necho "GNU Make %s"\n' "$3" >"$mb_d/make" && chmod 755 "$mb_d/make" && echo "$mb_d/make"
    return 0
  fi
  source_unpack "$mb_d" "$4" "$5" || return 0
  [ -x /usr/bin/make ] || {
    echo "error: /usr/bin/make (the package's make, which builds this one) is missing" >&2
    return 0
  }
  as_user timeout 600 sh -c 'cd "$1" && ./configure --disable-nls --without-guile && /usr/bin/make' sh "$mb_d/src" </dev/null >&2 || return 0
  # root copies the binary into the temporary directory (root's, the build user cannot write there)
  # and checks the copy: a symbolic link left by the build is copied as a link and refused.
  cp -P "$mb_d/src/make" "$mb_d/make" 2>/dev/null && chown root:root "$mb_d/make" &&
    [ -f "$mb_d/make" ] && [ ! -L "$mb_d/make" ] && [ -x "$mb_d/make" ] && echo "$mb_d/make"
}
p_make() {
  while read -r mn mv ms mu; do
    [ -n "$mn" ] || continue
    dir=/usr/local/$mn-$mv
    binok=1
    linkok=1
    real=
    for l in make gmake; do
      got=$(readlink "$R/usr/local/bin/$l" 2>/dev/null)
      [ "$got" = "$dir/bin/make" ] && continue
      drift "make" "/usr/local/bin/$l is '${got:-missing}', want a link to $dir/bin/make"
      linkok=
      if [ -e "$R/usr/local/bin/$l" ] && [ ! -L "$R/usr/local/bin/$l" ]; then real="$real /usr/local/bin/$l"; fi
    done
    v=$(as_user "$R$dir/bin/make" --version 2>/dev/null </dev/null | head -n 1)
    case $v in
      "GNU Make $mv" | "GNU Make $mv "*) ;;
      *)
        drift "make" "$dir/bin/make says '${v:-missing}', want GNU Make $mv"
        binok=
        ;;
    esac
    fixing && { [ -z "$binok" ] || [ -z "$linkok" ]; } || continue
    if [ -n "$real" ]; then
      failed "make" "${real# } is a real file; not replaced"
      continue
    fi
    if [ -n "$binok" ]; then
      if ln -sfn "$dir/bin/make" "$R/usr/local/bin/make" && ln -sfn "$dir/bin/make" "$R/usr/local/bin/gmake"; then
        changed "make" "linked make and gmake in /usr/local/bin to $dir/bin/make"
      else
        failed "make" "link in /usr/local/bin"
      fi
      continue
    fi
    if [ -n "$T" ]; then td=$(mktemp -d); else td=$(mktemp -d /var/tmp/slot-make.XXXXXX); fi || {
      failed "make" "no temporary directory"
      continue
    }
    ml=$(mktemp)
    mk=$(make_build "$td" "$mn" "$mv" "$ms" "$mu" 2>"$ml")
    if [ -z "$mk" ]; then
      failed "make" "download, checksum or build of make $mv: $({ grep -m 3 -i 'error' "$ml"; tail -n 2 "$ml"; } | tr '\n' ' ')"
    elif mkdir -p "$R$dir/bin" "$R/usr/local/bin" && install -m 0755 "$mk" "$R$dir/bin/make.slot-new" && mv "$R$dir/bin/make.slot-new" "$R$dir/bin/make" &&
      ln -sfn "$dir/bin/make" "$R/usr/local/bin/make" && ln -sfn "$dir/bin/make" "$R/usr/local/bin/gmake"; then
      v=$(as_user "$R$dir/bin/make" --version 2>/dev/null </dev/null | head -n 1)
      case $v in
        "GNU Make $mv" | "GNU Make $mv "*) changed "make" "GNU Make $mv built, linked as make and gmake in /usr/local/bin" ;;
        *) failed "make" "$dir/bin/make says '${v:-missing}' after the install" ;;
      esac
    else
      rm -f "$R$dir/bin/make.slot-new"
      failed "make" "$dir not written"
    fi
    rm -rf "$td" "$ml"
  done <<EOF
$MAKE_PIN
EOF
}

# dash built from its source tarball (DASH_PIN): Amazon Linux 2023 packages no dash, and the
# forge's runner image has dash as its sh, so a shell test that passes under a node's sh (bash) can
# fail there. Built like make: by the build user in a temporary directory with the package's make
# and no jobserver; root installs the one binary into /usr/local/dash-<version>/bin and links it
# as dash in /usr/local/bin (never as sh: a node's sh stays the distribution's). dash prints no
# version, so the tree carries the SHA-256 of the tarball it was built from (.slot-sha256, written
# by root), and the check also runs one command with the binary.
# Prints the path of the built dash; nothing when the download, the checksum or the build failed.
dash_build() {
  db_d=$1
  if fake_fetch; then
    [ -e "$R/.state-tool-fetch-fails" ] && return 0
    calls "fetch $5"
    printf '#!/bin/sh\nexec sh "$@"\n' >"$db_d/dash" && chmod 755 "$db_d/dash" && echo "$db_d/dash"
    return 0
  fi
  source_unpack "$db_d" "$4" "$5" || return 0
  [ -x /usr/bin/make ] || {
    echo "error: /usr/bin/make (the package's make, which builds dash) is missing" >&2
    return 0
  }
  as_user timeout 600 sh -c 'cd "$1" && ./configure && /usr/bin/make' sh "$db_d/src" </dev/null >&2 || return 0
  cp -P "$db_d/src/src/dash" "$db_d/dash" 2>/dev/null && chown root:root "$db_d/dash" &&
    [ -f "$db_d/dash" ] && [ ! -L "$db_d/dash" ] && [ -x "$db_d/dash" ] && echo "$db_d/dash"
}
# dash_answer PATH: what the dash at PATH prints for one arithmetic command, run as the build user.
dash_answer() { as_user timeout 10 "$1" -c 'echo $((6 * 7))' 2>/dev/null </dev/null | head -n 1; }
p_dash() {
  while read -r dn dv ds du; do
    [ -n "$dn" ] || continue
    dir=/usr/local/$dn-$dv
    binok=1
    linkok=1
    real=
    got=$(readlink "$R/usr/local/bin/dash" 2>/dev/null)
    if [ "$got" != "$dir/bin/dash" ]; then
      drift "dash" "/usr/local/bin/dash is '${got:-missing}', want a link to $dir/bin/dash"
      linkok=
      if [ -e "$R/usr/local/bin/dash" ] && [ ! -L "$R/usr/local/bin/dash" ]; then real=1; fi
    fi
    if [ -x "$R$dir/bin/dash" ] && [ ! -L "$R$dir/bin/dash" ]; then
      mark=$(cat "$R$dir/.slot-sha256" 2>/dev/null)
      if [ "$mark" != "$ds" ]; then
        drift "dash" "$dir is of the tarball '${mark:-unknown}', want $ds"
        binok=
      else
        v=$(dash_answer "$R$dir/bin/dash")
        if [ "$v" != 42 ]; then
          drift "dash" "$dir/bin/dash does not run a command (it answers '$v', want 42)"
          binok=
        fi
      fi
    else
      drift "dash" "$dir/bin/dash is missing, want dash $dv built from its source tarball"
      binok=
    fi
    fixing && { [ -z "$binok" ] || [ -z "$linkok" ]; } || continue
    if [ -n "$real" ]; then
      failed "dash" "/usr/local/bin/dash is a real file; not replaced"
      continue
    fi
    if [ -n "$binok" ]; then
      if ln -sfn "$dir/bin/dash" "$R/usr/local/bin/dash"; then
        changed "dash" "linked dash in /usr/local/bin to $dir/bin/dash"
      else
        failed "dash" "link in /usr/local/bin"
      fi
      continue
    fi
    if [ -n "$T" ]; then td=$(mktemp -d); else td=$(mktemp -d /var/tmp/slot-dash.XXXXXX); fi || {
      failed "dash" "no temporary directory"
      continue
    }
    dl=$(mktemp)
    db=$(dash_build "$td" "$dn" "$dv" "$ds" "$du" 2>"$dl")
    if [ -z "$db" ]; then
      failed "dash" "download, checksum or build of dash $dv: $({ grep -m 3 -i 'error' "$dl"; tail -n 2 "$dl"; } | tr '\n' ' ')"
    elif mkdir -p "$R$dir/bin" "$R/usr/local/bin" && install -m 0755 "$db" "$R$dir/bin/dash.slot-new" && mv "$R$dir/bin/dash.slot-new" "$R$dir/bin/dash" &&
      printf '%s\n' "$ds" >"$R$dir/.slot-sha256" && ln -sfn "$dir/bin/dash" "$R/usr/local/bin/dash"; then
      v=$(dash_answer "$R$dir/bin/dash")
      if [ "$v" = 42 ]; then
        changed "dash" "dash $dv built, linked as dash in /usr/local/bin"
      else
        failed "dash" "$dir/bin/dash answers '$v' after the install, want 42"
      fi
    else
      rm -f "$R$dir/bin/dash.slot-new"
      failed "dash" "$dir not written"
    fi
    rm -rf "$td" "$dl"
  done <<EOF
$DASH_PIN
EOF
}

# musl built from its source tarball (MUSL_PIN): Amazon Linux 2023 packages no musl-gcc, and a
# build that names it as the C compiler of the musl target (CC_x86_64_unknown_linux_musl=musl-gcc)
# stopped with `failed to find tool "musl-gcc"`. The build user configures the source for the
# prefix /usr/local/musl-<version>, with the loader's link in the prefix too (--syslibdir: the
# default is /lib, outside it), builds it with 4 make jobs and installs it into a directory of
# its own source tree (DESTDIR). Root copies that tree, so nothing of the build runs as root and
# no installed file is the build user's. bin/musl-gcc is the wrapper (--enable-wrapper=gcc) that
# runs the node's gcc with lib/musl-gcc.specs; it is linked as /usr/local/bin/musl-gcc.
# Measured on a node of 32 CPUs at a load of 58 to 68: configure 1 s, make 104 s with one job and
# 31 s with 4, make install 3 to 8 s.
# Prints the path of the tree to install; nothing when the download, the checksum or the build
# failed. Arguments: dir name version sha256 url prefix. NODE_TEST=1: a tree of stand-ins, or
# nothing when $R/.state-tool-fetch-fails exists.
musl_build() {
  ub_d=$1
  if fake_fetch; then
    [ -e "$R/.state-tool-fetch-fails" ] && return 0
    calls "fetch $5"
    mkdir -p "$ub_d/tree/bin" "$ub_d/tree/lib" "$ub_d/tree/include" || return 0
    # Like the wrapper, `--version` answers as a compiler does. In place of a compile it writes,
    # as the file that -o names, a script that prints what the puts line of the source prints.
    # Like the loader, libc.so prints its release on standard error and exits 1.
    cat >"$ub_d/tree/bin/musl-gcc" <<'EOF'
#!/bin/sh
[ "$1" != --version ] || { echo "gcc (stand-in of provision.sh under NODE_TEST=1)"; exit 0; }
o=
s=
while [ $# -gt 0 ]; do
  case $1 in
    -o) o=$2; shift ;;
    *.c) s=$1 ;;
  esac
  shift
done
[ -n "$o" ] && [ -f "$s" ] || exit 1
printf '#!/bin/sh\necho "%s"\n' "$(sed -n 's/.*puts("\(.*\)").*/\1/p' "$s")" >"$o" && chmod 755 "$o"
EOF
    printf '#!/bin/sh\necho "musl libc (x86_64)" >&2\necho "Version %s" >&2\nexit 1\n' "$3" >"$ub_d/tree/lib/libc.so"
    : >"$ub_d/tree/lib/musl-gcc.specs"
    : >"$ub_d/tree/lib/libc.a"
    : >"$ub_d/tree/include/stdio.h"
    chmod -R go-w "$ub_d/tree" && chmod -R a+rX "$ub_d/tree" && chmod 755 "$ub_d/tree/bin/musl-gcc" "$ub_d/tree/lib/libc.so" && echo "$ub_d/tree"
    return 0
  fi
  source_unpack "$ub_d" "$4" "$5" || return 0
  [ -x /usr/bin/make ] || {
    echo "error: /usr/bin/make (the package's make, which builds musl) is missing" >&2
    return 0
  }
  as_user timeout 600 sh -c 'cd "$1" && ./configure --prefix="$2" --syslibdir="$2/lib" --enable-wrapper=gcc && /usr/bin/make -j4 && /usr/bin/make install DESTDIR="$1/.slot-stage"' sh "$ub_d/src" "$6" </dev/null >&2 || return 0
  # The copy is root's: made without the owner or the set-id bits of the build's files, a link
  # copied as a link. A link in the path of the installed tree, or in place of one of the files a
  # compile needs, refuses the whole tree.
  ub_s=$ub_d/src/.slot-stage$6
  if ! no_link "$ub_s" || [ ! -d "$ub_s" ]; then
    echo "error: the build of musl left no directory $6 under its DESTDIR" >&2
    return 0
  fi
  cp -RP "$ub_s" "$ub_d/tree" || return 0
  chmod -R go-w,u-s,g-s "$ub_d/tree" && chmod -R a+rX "$ub_d/tree" || return 0
  for ub_f in bin/musl-gcc lib/musl-gcc.specs lib/libc.a lib/libc.so include/stdio.h; do
    if [ ! -f "$ub_d/tree/$ub_f" ] || [ -L "$ub_d/tree/$ub_f" ]; then
      echo "error: the build of musl left no file $ub_f" >&2
      return 0
    fi
  done
  echo "$ub_d/tree"
}
# What the loader $1 names as its release, run as the build user: the line `Version <release>`
# among the lines it prints on standard error (it exits 1). Nothing for a file that prints none.
musl_says() {
  as_user "$1" </dev/null 2>&1 | sed -n '/^Version /p' | head -n 1
}
p_musl() {
  while read -r un uv us uu; do
    [ -n "$un" ] || continue
    dir=/usr/local/$un-$uv
    ul=/usr/local/bin/musl-gcc
    binok=1
    linkok=1
    got=$(readlink "$R$ul" 2>/dev/null)
    if [ "$got" != "$dir/bin/musl-gcc" ]; then
      drift "musl" "$ul is '${got:-missing}', want a link to $dir/bin/musl-gcc"
      linkok=
    fi
    v=$(musl_says "$R$dir/lib/libc.so")
    if [ "$v" != "Version $uv" ]; then
      drift "musl" "$dir/lib/libc.so says '${v:-missing}', want Version $uv"
      binok=
    elif [ -z "$(as_user "$R$dir/bin/musl-gcc" --version 2>/dev/null </dev/null | head -n 1)" ]; then
      drift "musl" "$dir/bin/musl-gcc --version answers nothing"
      binok=
    fi
    fixing && { [ -z "$binok" ] || [ -z "$linkok" ]; } || continue
    if [ -e "$R$ul" ] && [ ! -L "$R$ul" ]; then
      failed "musl" "$ul is a real file; not replaced"
      continue
    fi
    if [ -n "$binok" ]; then
      if ln -sfn "$dir/bin/musl-gcc" "$R$ul"; then
        changed "musl" "linked musl-gcc in /usr/local/bin to $dir/bin/musl-gcc"
      else
        failed "musl" "link in /usr/local/bin"
      fi
      continue
    fi
    if [ -n "$T" ]; then td=$(mktemp -d); else td=$(mktemp -d /var/tmp/slot-musl.XXXXXX); fi || {
      failed "musl" "no temporary directory"
      continue
    }
    ml=$(mktemp)
    src=$(musl_build "$td" "$un" "$uv" "$us" "$uu" "$dir" 2>"$ml" </dev/null)
    if [ -z "$src" ]; then
      failed "musl" "download, checksum or build of musl $uv: $({ grep -m 3 -i 'error' "$ml"; tail -n 2 "$ml"; } | tr '\n' ' ')"
    elif mkdir -p "$R/usr/local/bin" && rm -rf "$R$dir.slot-new" && mv "$src" "$R$dir.slot-new" && rm -rf "$R$dir" && mv "$R$dir.slot-new" "$R$dir" &&
      ln -sfn "$dir/bin/musl-gcc" "$R$ul"; then
      v=$(musl_says "$R$dir/lib/libc.so")
      if [ "$v" = "Version $uv" ]; then
        changed "musl" "musl $uv built in $dir, linked as musl-gcc in /usr/local/bin"
      else
        failed "musl" "$dir/lib/libc.so says '${v:-missing}' after the install"
      fi
    else
      rm -rf "$R$dir.slot-new"
      failed "musl" "$dir not written"
    fi
    rm -rf "$td" "$ml"
  done <<EOF
$MUSL_PIN
EOF
}
# check --canary: what a job does with musl-gcc. The build user writes one C file that includes
# stdio.h into a new directory of the store's tmp (the TMPDIR of a job), compiles it with
# `musl-gcc -static`, runs the result and removes the directory. The script answers one line:
# `out <what the program printed>`, `cc <the compiler's first line with "error", else its first
# line>`, or `tmp` without a directory.
p_musl_canary() {
  [ -n "$canary" ] && [ "$mode" = check ] || return 0
  while read -r un uv us uu; do
    [ -n "$un" ] || continue
    mc_w='slot musl canary'
    mc_o=$(as_user timeout 60 sh -c 'd=$(mktemp -d "$2/slot-musl-canary.XXXXXX" 2>/dev/null) || { echo tmp; exit 0; }
if cd "$d"; then
  printf "#include <stdio.h>\nint main(void) { puts(\"%s\"); return 0; }\n" "$3" >canary.c
  if "$1" -static -o canary canary.c >log 2>&1; then
    echo "out $(./canary 2>&1 | head -n 1)"
  else
    e=$(grep -m 1 "error" log)
    [ -n "$e" ] || e=$(head -n 1 log)
    echo "cc ${e:-no line from the compiler}"
  fi
fi
cd /
rm -rf "$d"' sh "$R/usr/local/$un-$uv/bin/musl-gcc" "$R$STORE/tmp" "$mc_w" </dev/null 2>&1)
    case $mc_o in
      "out $mc_w") echo "canary: ok: musl: a C file that includes stdio.h, compiled with musl-gcc -static (musl $uv), runs and prints its line" ;;
      "out "*) drift "musl" "canary: the program that musl-gcc -static made prints '${mc_o#out }', want '$mc_w'" ;;
      "cc "*) drift "musl" "canary: musl-gcc -static canary.c failed: ${mc_o#cc }" ;;
      tmp*) drift "musl" "canary: the build user got no directory in $STORE/tmp: nothing was compiled" ;;
      *) drift "musl" "canary: no answer from the compile within 60 s: nothing was tested" ;;
    esac
  done <<EOF
$MUSL_PIN
EOF
}
# check --canary with EXTRA_ARCHIVES=firefox: what a consumer's browser tests do with firefox. The
# build user starts it from its tree in a new directory of the store's tmp (the directory holds its
# HOME, TMPDIR and profile), headless with a WebDriver BiDi port, waits up to 30 s for the line
# `WebDriver BiDi listening on ws://...` on standard error, ends it and removes the directory. The
# script answers one line: `bidi <that line>`, `exit <status> <line>` when firefox ended before it,
# `none <line>` when it runs without it, or `tmp` without a directory. <line> is the first line of
# standard error that is not the notice of headless mode, else that notice.
p_firefox_canary() {
  [ -n "$canary" ] && [ "$mode" = check ] || return 0
  fc_v=$(archive_pins | awk '$1 == "firefox" { print $2 }')
  [ -n "$fc_v" ] || return 0
  fc_t=30
  [ -z "$T" ] || fc_t=${NODE_TEST_FIREFOX_WAIT:-$fc_t}
  fc_o=$(as_user timeout $((fc_t + 30)) sh -c 'd=$(mktemp -d "$2/slot-firefox-canary.XXXXXX" 2>/dev/null) || { echo tmp; exit 0; }
first() { l=$(grep -v -m 1 "^\*\*\* You are running in headless mode" "$d/err"); [ -n "$l" ] || l=$(head -n 1 "$d/err"); echo "${l:-no line on standard error}"; }
if cd "$d" && mkdir home tmp profile; then
  HOME=$d/home TMPDIR=$d/tmp MOZ_HEADLESS=1 "$1" --headless --no-remote --remote-debugging-port 0 --profile "$d/profile" about:blank >"$d/out" 2>"$d/err" </dev/null &
  p=$!
  i=0
  l=
  while [ "$i" -lt $(($3 * 5)) ]; do
    l=$(grep -m 1 "^WebDriver BiDi listening on ws://" "$d/err") && break
    kill -0 "$p" 2>/dev/null || break
    sleep 0.2
    i=$((i + 1))
  done
  [ -n "$l" ] || l=$(grep -m 1 "^WebDriver BiDi listening on ws://" "$d/err")
  if [ -n "$l" ]; then
    echo "bidi $l"
  elif kill -0 "$p" 2>/dev/null; then
    echo "none $(first)"
  else
    wait "$p"
    echo "exit $? $(first)"
  fi
  kill "$p" 2>/dev/null
  wait "$p" 2>/dev/null
fi
cd /
rm -rf "$d"' sh "$R/usr/local/firefox-$fc_v/firefox" "$R$STORE/tmp" "$fc_t" </dev/null 2>&1)
  case $fc_o in
    "bidi WebDriver BiDi listening on "*) echo "canary: ok: firefox: firefox $fc_v started headless as the build user and listened for WebDriver BiDi (${fc_o#bidi WebDriver BiDi listening on })" ;;
    "exit "*)
      fc_r=${fc_o#exit }
      drift firefox "canary: firefox exited with status ${fc_r%% *} before it listened for WebDriver BiDi: ${fc_r#* }"
      ;;
    "none "*) drift firefox "canary: no WebDriver BiDi line from firefox within $fc_t s: ${fc_o#none }" ;;
    tmp*) drift firefox "canary: the build user got no directory in $STORE/tmp: nothing was started" ;;
    *) drift firefox "canary: no answer from the start within $((fc_t + 30)) s: nothing was tested" ;;
  esac
}

# The trees of HOME_PINS. Downloads the archive into directory $1, checks its SHA-256, unpacks it
# into $1/tree and prints that path. Arguments: dir version sha256 url strip check. NODE_TEST=1:
# a stub tree, or nothing when $R/.state-tool-fetch-fails exists.
home_fetch() {
  hd=$1 hv=$2 hs=$3 hu=$4 hk=$5 hc=$6
  if fake_fetch; then
    [ -e "$R/.state-tool-fetch-fails" ] && return 0
    mkdir -p "$hd/tree" || return 0
    case $hc in
      package.json) printf '{\n  "name": "stub",\n  "version": "%s"\n}\n' "$hv" >"$hd/tree/package.json" ;;
      *chrome-headless-shell) mkdir -p "$hd/tree/${hc%/*}" && printf '#!/bin/sh\necho "Google Chrome for Testing %s"\n' "$hv" >"$hd/tree/$hc" && chmod 755 "$hd/tree/$hc" ;;
      *.jar) mkdir -p "$hd/tree/$(dirname "$hc")" && echo stub >"$hd/tree/$hc" ;;
      *) printf '#!/bin/sh\necho "%s %s"\n' "$hc" "$hv" >"$hd/tree/$hc" && chmod 755 "$hd/tree/$hc" ;;
    esac
    calls "fetch $hu"
  elif [ "$hk" = zip ]; then
    (cd "$hd" && curl -fsSLo archive "$hu" && echo "$hs  archive" | sha256sum -c --strict - >/dev/null &&
      mkdir tree && unzip -q archive -d tree) || return 0
  elif [ "$hk" = zip1 ]; then
    # A zip with one top directory: that directory becomes the tree. Anything else at the top (no
    # entry, more than one, a file, a symbolic link) is refused. `ls` prints the name as it is,
    # whatever QUOTING_STYLE the caller exports.
    (cd "$hd" && curl -fsSLo archive "$hu" && echo "$hs  archive" | sha256sum -c --strict - >/dev/null &&
      mkdir x && unzip -q archive -d x && [ "$(QUOTING_STYLE=literal ls -A x | wc -l)" = 1 ] && top=$(QUOTING_STYLE=literal ls -A x) &&
      [ -d "x/$top" ] && [ ! -L "x/$top" ] && mv "x/$top" tree) || return 0
  else
    (cd "$hd" && curl -fsSLo archive "$hu" && echo "$hs  archive" | sha256sum -c --strict - >/dev/null &&
      mkdir tree && tar -xzf archive -C tree --strip-components="$hk" --no-same-owner) || return 0
  fi
  # A zip is a Playwright browser: its directory needs the two marker files Playwright looks for.
  if [ "$hk" = zip ]; then : >"$hd/tree/INSTALLATION_COMPLETE" && : >"$hd/tree/DEPENDENCIES_VALIDATED" || return 0; fi
  chmod -R u+rwX,go+rX,go-w,u-s,g-s "$hd/tree"
  echo "$hd/tree"
}
# What the check of a HOME_PINS line reads in directory $1: `<binary> <version>` for a binary, the
# `version` of package.json for package.json, the last word of the first line of `--version` for a
# chrome-headless-shell, the version $3 for a `.jar` that exists as a regular file.
home_got() {
  case $2 in
    *.jar) if [ -f "$R$1/$2" ] && [ ! -L "$R$1/$2" ]; then echo "$3"; fi ;;
    package.json) sed -n 's/^[[:space:]]*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$R$1/package.json" 2>/dev/null | head -n 1 ;;
    *chrome-headless-shell) (cd / && as_user "$R$1/$2" --version 2>/dev/null </dev/null | head -n 1 | awk '{print $NF}') ;;
    *) tool_version "$R$1/$2" ;;
  esac
}
p_home_pins() {
  while read -r hn hv hs hu hk hd hc; do
    [ -n "$hn" ] || continue
    p=$H/$hd
    case $hc in package.json | *chrome-headless-shell | *.jar) want=$hv ;; *) want="$hc $hv" ;; esac
    got=$(home_got "$p" "$hc" "$hv")
    mark=$(cat "$R$p/.slot-sha256" 2>/dev/null)
    [ "$got" = "$want" ] && [ "$mark" = "$hs" ] && continue
    drift "home:$hn@$hv" "$p is '${got:-missing}', want $want$([ "$got" = "$want" ] && echo " from the archive $hs")"
    fixing || continue
    if ! no_link "$p"; then
      failed "home:$hn@$hv" "$p has a symbolic link in it; not installed"
      continue
    fi
    if [ -n "$T" ]; then td=$(mktemp -d); else td=$(mktemp -d /var/tmp/slot-home.XXXXXX); fi || {
      failed "home:$hn@$hv" "no temporary directory"
      continue
    }
    chmod 755 "$td"
    src=$(home_fetch "$td" "$hv" "$hs" "$hu" "$hk" "$hc" </dev/null)
    if [ -z "$src" ]; then
      failed "home:$hn@$hv" "download or checksum of $hn $hv"
    elif as_user sh -c 'set -e; mkdir -p "$(dirname "$2")"; rm -rf "$2.slot-new" "$2.slot-old"; mkdir "$2.slot-new"; cp -a "$1/." "$2.slot-new/"; printf "%s\n" "$3" >"$2.slot-new/.slot-sha256"; if [ -e "$2" ]; then mv "$2" "$2.slot-old"; fi; mv "$2.slot-new" "$2"; rm -rf "$2.slot-old"' sh "$src" "$R$p" "$hs" </dev/null; then
      got=$(home_got "$p" "$hc" "$hv")
      mark=$(cat "$R$p/.slot-sha256" 2>/dev/null)
      if [ "$got" = "$want" ] && [ "$mark" = "$hs" ]; then
        changed "home:$hn@$hv" "$hn $hv installed in $p"
      else
        failed "home:$hn@$hv" "$p is '${got:-missing}' after the install"
      fi
    else
      as_user rm -rf "$R$p.slot-new" </dev/null
      failed "home:$hn@$hv" "$p not written"
    fi
    rm -rf "$td"
  done <<EOF
$HOME_PINS
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

# Core dumps. A bare node has kernel.core_pattern as a pipe to systemd-coredump, and the kernel
# runs a pipe handler whatever the core limit of the process is: a process killed by SIGABRT was
# reaped only after the handler, 142 to 256 ms later on an idle node and up to 3.5 s with 24 at
# once, and a consumer's tests that wait a few seconds for an aborted child timed out. The node's
# pattern is the file name `core`: no handler runs, and under the soft core limit 0
# (LimitCORE=0:infinity of slot-node.service and sccache-node.service; a job that wants a core
# file raises its own limit) nothing is written.
#
# p_core: the sysctl file, which sets the pattern at every boot. A pattern that is neither a pipe
# nor `core` is the owner's choice and stays: with no file there none is written, and one line
# says so. A file that is already there stays as it is and makes the pattern `core` at the next
# boot.
p_core() {
  co_v=$(q_core_pattern)
  case $co_v in
    "|"* | core) ;;
    *)
      if [ ! -e "$R$CORE_SYSCTL" ] && [ ! -L "$R$CORE_SYSCTL" ]; then
        echo "kept: core: kernel.core_pattern is '$co_v', neither a pipe nor core: it stays, and $CORE_SYSCTL is not written"
        return 0
      fi
      ;;
  esac
  want_file core "$CORE_SYSCTL" 644 root:root gen_core_sysctl
}

# p_core_load, after p_services: the pattern of the running kernel. Under the pattern `core` a
# process whose core limit is not 0 writes a core file into its working directory, so the file is
# loaded only when sccache-node and slot-node run with the soft limit 0: p_services restarts a
# unit with another limit when no job runs, and until then the pipe stays.
p_core_load() {
  co_v=$(q_core_pattern)
  case $co_v in
    "|"*) ;;
    *) return 0 ;;
  esac
  drift core "kernel.core_pattern is '$co_v', a pipe: an aborted process waits for the handler"
  fixing || return 0
  [ -e "$R$CORE_SYSCTL" ] || return 0
  for co_u in sccache-node.service slot-node.service; do
    q_active "$co_u" || continue
    co_l=$(q_core_limit "$co_u")
    [ "$co_l" = 0 ] && continue
    # During a drain no apply is needed for it: the start that ends the drain loads the file
    # (gap_core_load).
    co_t="the apply that restarts it loads $CORE_SYSCTL"
    [ ! -f "$R$DRAIN_DIR/drain" ] || co_t="the start of slot-node.service at the end of its drain loads $CORE_SYSCTL"
    echo "pending: core: kernel.core_pattern stays '$co_v' while $co_u runs with the core limit ${co_l:-unknown} (under the pattern core its processes write core files): $co_t" >&2
    return 0
  done
  a_sysctl_load "$CORE_SYSCTL"
  co_v=$(q_core_pattern)
  case $co_v in
    "|"*) failed core "kernel.core_pattern is still '$co_v' after sysctl -p $CORE_SYSCTL" ;;
    *) changed core "kernel.core_pattern is '$co_v' (sysctl -p $CORE_SYSCTL)" ;;
  esac
}

# check --canary: an aborted process under the soft core limit 0 is reaped within 50 ms
# (q_abort_ms), and slot-node.service, whose limit its jobs inherit, runs with that limit. The
# time is compared as a number only when it has at most 6 digits: a longer one is over the bound
# and outside what the shell compares.
p_core_canary() {
  [ -n "$canary" ] && [ "$mode" = check ] || return 0
  [ -z "${draining:-}" ] || return 0
  co_ms=$(q_abort_ms)
  case $co_ms in
    '' | *[!0-9]*)
      drift core "canary: no time was measured for an aborted process: nothing was tested"
      return 0
      ;;
    ???????*) co_over=1 ;;
    *) if [ "$co_ms" -gt 50 ]; then co_over=1; else co_over=; fi ;;
  esac
  if [ -n "$co_over" ]; then
    drift core "canary: an aborted process is reaped after $co_ms ms (the quickest of 5), over 50"
    return 0
  fi
  if ! q_active slot-node.service; then
    drift core "canary: slot-node.service is not active: the core limit of its jobs was not read"
    return 0
  fi
  co_l=$(q_core_limit slot-node.service)
  if [ "$co_l" != 0 ]; then
    drift core "canary: slot-node.service runs with the core limit '${co_l:-unknown}', not 0: an aborted process of a job writes a core file"
    return 0
  fi
  echo "canary: ok: core: an aborted process is reaped in $co_ms ms (the quickest of 5), and slot-node.service runs with the core limit 0"
}

# docker for the jobs that start containers (a consumer's OCI lane). The package is one of
# BASE_PACKAGES, docker.service one of ENABLED, and p_services starts it behind
# slot-imds-containers.service. Here: the build user is in group docker, which is what lets a job
# reach the daemon's socket. A member of that group is root on the node (a privileged or
# host-network container), so the rule that only root reaches the metadata service holds against
# the build user's own processes and against bridge containers, not against a job that starts
# such a container. slot-node gets the group when it starts: stale_reason names a process that
# started before.
#
# The metadata service gives the instance role's credentials to root, so a job gets docker only
# where those are worth nothing to it: the instance profile is the node's own (role.sh makes
# slot-node-<name>: one policy, the channels of the Systems Manager agent) or the instance has
# none, and the service wants a token (IMDSv2: a URL fetch of the docker daemon cannot send
# one). p_role reads both from the metadata service. It calls no AWS API and changes nothing:
# role.sh does, where the owner's identity is. Until then the build user gets no group docker,
# and a build user that has it is a failed line.
role_why=
p_role() {
  # A machine outside EC2 has no metadata service to ask.
  if [ "$WANT_PROFILE" = none ] && ! q_ec2; then return 0; fi
  pr_p=$(q_instance_profile)
  case $pr_p in
    '(none)') ;;
    '') role_why="the metadata service does not name the instance profile" ;;
    *)
      if [ "$WANT_PROFILE" = none ]; then
        role_why="the instance profile is '$pr_p', and the config says INSTANCE_PROFILE=none"
      elif [ -z "$WANT_PROFILE" ]; then
        role_why="the instance profile is '$pr_p' and the config names none (NODE_NAME or INSTANCE_PROFILE)"
      elif [ "$pr_p" != "$WANT_PROFILE" ]; then
        role_why="the instance profile is '$pr_p', not $WANT_PROFILE (role.sh apply makes it)"
      elif [ -z "$INSTANCE_PROFILE" ]; then
        # role.sh gives the profile a role of the same name. A profile the config names itself
        # (INSTANCE_PROFILE) is the owner's word: its role is not compared.
        pr_r=$(q_instance_role)
        [ "$pr_r" = "$WANT_PROFILE" ] || role_why="the instance profile $WANT_PROFILE gives out the role '${pr_r:-unknown}', not the role of its name (role.sh check names the difference)"
      fi
      ;;
  esac
  if [ -z "$role_why" ]; then
    case $(q_imds_v1) in
      off) ;;
      on) role_why="the metadata service answers without a token (role.sh apply sets tokens required)" ;;
      *) role_why="the metadata service neither refused nor answered a request without a token" ;;
    esac
  fi
  [ -n "$role_why" ] || return 0
  drift role "$role_why"
  fixing || return 0
  case " $(q_groups "$U") " in
    *" docker "*) failed role "$role_why: $U is in group docker, and a container of a job reaches the credentials of this role (gpasswd -d $U docker and a restart of slot-node.service take the group away)" ;;
    *) failed role "$role_why: $U gets no group docker" ;;
  esac
}

# check --canary: one container tries the role (role-canary.sh). An answer other than a refusal
# is drift. NODE_TEST=1 changes nothing on the host, so there the canary runs only with the
# stand-in for docker that the test names: NODE_TEST_DOCKER, the directory that holds it. Without
# one nothing is started, whatever `docker` PATH has.
p_canary() {
  [ -n "$canary" ] && [ "$mode" = check ] || return 0
  if ! q_active docker.service; then
    drift role "canary: docker.service is not active: nothing was tested"
    return 0
  fi
  pc_path=$PATH
  if [ -n "$T" ]; then
    if [ -z "${NODE_TEST_DOCKER:-}" ] || [ ! -x "$NODE_TEST_DOCKER/docker" ]; then
      drift role "canary: NODE_TEST=1 names no stand-in for docker (NODE_TEST_DOCKER): nothing was started"
      return 0
    fi
    pc_path=$NODE_TEST_DOCKER:$PATH
  fi
  pc_f=$(mktemp) || {
    drift role "canary: no temporary file: nothing was tested"
    return 0
  }
  gen_role_canary >"$pc_f"
  if pc_o=$(PATH=$pc_path sh "$pc_f" 2>&1); then
    echo "canary: $pc_o"
  else
    drift role "canary: ${pc_o#role canary: }"
  fi
  rm -f "$pc_f"
}

p_docker() {
  [ -z "$role_why" ] || return 0
  case " $(q_groups "$U") " in *" docker "*) return 0 ;; esac
  drift docker "$U is not in group docker"
  fixing || return 0
  if [ -n "$T" ]; then
    st_add groups "$U:docker"
  else
    { getent group docker >/dev/null || groupadd -r docker; } && usermod -aG docker "$U" || {
      failed docker "usermod -aG docker $U"
      return 0
    }
  fi
  changed docker "$U added to group docker"
}

# slot-imds-containers.service stays active after its rules are gone (a flushed chain), so the
# rules are read from the firewall, like the rule of slot-imds-root-only.service. They are put
# back here and not by a restart of the unit: docker.service requires it and would restart too,
# which ends every container.
p_docker_rule() {
  q_active slot-imds-containers.service || return 0
  q_imds_containers && return 0
  drift docker "slot-imds-containers.service is active and its reject rules are not in DOCKER-USER"
  fixing || return 0
  if [ -n "$T" ]; then
    calls "iptables -w -I DOCKER-USER 1 -d 169.254.169.254/32 -j REJECT"
    [ -e "$R/.state-imds-containers-stay-open" ] || : >"$S/imds-containers"
  else
    for pr in 'iptables 169.254.169.254/32' 'ip6tables fd00:ec2::254/128'; do
      "${pr% *}" -w -N DOCKER-USER 2>/dev/null
      "${pr% *}" -w -C DOCKER-USER -d "${pr#* }" -j REJECT 2>/dev/null ||
        "${pr% *}" -w -I DOCKER-USER 1 -d "${pr#* }" -j REJECT
    done
  fi
  if q_imds_containers; then
    changed docker "reject rules for the metadata service put back into DOCKER-USER"
  else
    failed docker "the reject rules for the metadata service are not in DOCKER-USER"
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
  # A process that started before its unit had LimitCORE (p_core_load says why the limit counts).
  sr_l=$(q_core_limit "$1")
  if [ -n "$sr_l" ] && [ "$sr_l" != 0 ]; then
    echo "the process has the core limit $sr_l, not 0"
    return 0
  fi
  if [ "$1" = slot-node.service ]; then
    sr_v=$(q_running_version "$1")
    if [ -n "$sr_v" ] && [ "$sr_v" != "slot $SLOT_VERSION" ]; then
      echo "it runs '$sr_v', the config says slot $SLOT_VERSION"
      return 0
    fi
    # The same version of the other build: the binary at the path was replaced by an apply whose
    # restart waited for jobs.
    sr_t=$(q_running_target "$1")
    if [ -n "$sr_t" ] && [ "$sr_t" != "$SLOT_TARGET" ]; then
      echo "it runs the slot build for $sr_t, the config says $SLOT_TARGET"
      return 0
    fi
    # A job is a child of this process and has its groups. Under a role that is not the node's
    # own (p_role) a restart for the group would hand docker to the jobs: it waits.
    sr_g=$(q_proc_groups "$1")
    [ -z "$role_why" ] || sr_g=docker
    case " $(echo $sr_g) " in
      "  " | *" docker "*) ;;
      *)
        case " $(q_groups "$U") " in
          *" docker "*)
            echo "the process started before $U was in group docker"
            return 0
            ;;
        esac
        ;;
    esac
  fi
  return 0
}

# Why the running unit $1 must restart: stale_reason, a unit file this run changed, the slot
# binary this run replaced, or other arguments than the config's. Nothing printed: no reason.
unit_why() {
  uw=$(stale_reason "$1")
  if [ -z "$uw" ]; then
    case " $units_changed " in *" $1 "*) fixing && uw="its unit file changed" ;; esac
    [ "$1" = slot-node.service ] && [ -n "$slot_replaced" ] && uw="the slot binary was replaced"
  fi
  if [ -z "$uw" ] && [ "$1" = slot-node.service ]; then
    uw_want=$(serve_args)
    uw_got=$(q_running_args "$1")
    [ "$uw_got" = "$uw_want" ] || uw="it runs '$uw_got', its unit says '$uw_want'"
  fi
  [ -z "$uw" ] || echo "$uw"
}

# A drain of slot-node.service, for a node that never has 0 jobs (apply --drain). The daemon
# alone gets SIGTERM (a_signal_main): it removes its socket, so it takes no new job, waits until
# the jobs it has have ended, and exits. systemd then starts the unit again (Restart=always
# covers an exit with status 0), with the unit file as the manager has it loaded and the groups
# the build user has at that moment; drain_start reloads the manager first. While it drains the
# node takes no job: a `slot` call there waits for the socket.
#
# The marks, files of root in DRAIN_DIR: `drain`, with the main process that got the signal
# (q_main_id), the time of the bound in seconds since the epoch, and on its second line the
# reason; `sccache-restart`, with the reason of a restart of sccache-node.service that waits.
# The drain runs while the main process of slot-node.service is the one of the mark.
#
# drain_read: dr_id, dr_until and dr_why from the mark, and draining=1 while the drain runs.
drain_read() {
  dr_id=
  dr_until=
  dr_why=
  draining=
  [ -f "$R$DRAIN_DIR/drain" ] || return 0
  {
    read -r dr_id dr_until
    read -r dr_why
  } <"$R$DRAIN_DIR/drain"
  case $dr_until in '' | *[!0-9]* | ????????????*) dr_until=0 ;; esac
  [ -n "$dr_id" ] && [ "$dr_id" = "$(q_main_id slot-node.service)" ] && draining=1
  return 0
}
# Starts a drain: the mark, the bound, then the signal. Without the bound nothing is signalled,
# and a signal that fails leaves neither mark nor bound. why_sn and why_sc are the reasons of the
# two units, j the jobs counted.
drain_start() {
  ds_id=$(q_main_id slot-node.service)
  if [ -z "$ds_id" ]; then
    failed drain "slot-node.service has no main process: nothing was signalled"
    return 0
  fi
  ds_why=${why_sn:-sccache-node.service restarts: $why_sc}
  ds_s=$(conf_seconds "$DRAIN_MAX")
  ds_until=$(($(q_now) + ds_s))
  a_daemon_reload
  rm -f "$R$DRAIN_DIR/sccache-restart"
  if ! { mkdir -p "$R$DRAIN_DIR" && printf '%s %s\n%s\n' "$ds_id" "$ds_until" "$ds_why" >"$R$DRAIN_DIR/drain.new" &&
    mv "$R$DRAIN_DIR/drain.new" "$R$DRAIN_DIR/drain"; }; then
    failed drain "$DRAIN_DIR/drain could not be written: slot-node.service was not signalled"
    return 0
  fi
  if ! a_drain_arm "$ds_s"; then
    rm -f "$R$DRAIN_DIR/drain"
    failed drain "the bound could not be armed (systemd-run --unit=$DRAIN_UNIT): slot-node.service was not signalled"
    return 0
  fi
  [ -z "$why_sc" ] || echo "$why_sc" >"$R$DRAIN_DIR/sccache-restart"
  if ! a_signal_main slot-node.service; then
    a_drain_disarm
    rm -f "$R$DRAIN_DIR/drain" "$R$DRAIN_DIR/sccache-restart"
    failed drain "systemctl kill --kill-whom=main --signal=TERM slot-node.service failed: nothing drains"
    return 0
  fi
  [ -z "$why_sc" ] || echo "pending: running: sccache-node restarts when slot-node does, at the end of its drain: $why_sc" >&2
  echo "draining: slot-node: ${j:-?} job(s), restart by $(utc_of "$ds_until")"
}
# The step before each start of slot-node.service (mode start-pre, the unit's ExecStartPre): no
# daemon of the pool runs now, whoever started the unit. A restart of sccache-node.service that
# waited happens here, and the drain is over: its mark and its bound go. The lines go to the
# journal of the unit.
gap_step() {
  if [ -f "$R$DRAIN_DIR/sccache-restart" ]; then
    gs_why=$(head -n 1 "$R$DRAIN_DIR/sccache-restart")
    rm -f "$R$DRAIN_DIR/sccache-restart"
    if a_restart sccache-node.service; then
      echo "slot-node: sccache-node.service restarted before slot-node.service starts: $gs_why"
    else
      echo "slot-node: systemctl restart sccache-node.service failed before the start of slot-node.service: the next apply restarts it ($gs_why)"
    fi
  fi
  if [ -f "$R$DRAIN_DIR/drain" ]; then
    rm -f "$R$DRAIN_DIR/drain"
    a_drain_disarm
    echo "slot-node: the drain is over: slot-node.service starts"
    gap_core_load
  fi
  return 0
}
# The end of a drain, in the step before the start: the pattern of the running kernel, which
# p_core_load leaves a pipe while a unit runs with another core limit than 0. `apply --drain` ends
# before the units restart, so without this the pipe stayed until a second apply. The condition
# is p_core_load's: the pattern is a pipe, the sysctl file is there, sccache-node (restarted above
# when its restart waited) runs with the soft limit 0, and slot-node, of which no process runs
# now, gets the soft limit 0 at this start. A start that ends no drain loads nothing.
gap_core_load() {
  gc_v=$(q_core_pattern)
  case $gc_v in
    "|"*) ;;
    *) return 0 ;;
  esac
  [ -e "$R$CORE_SYSCTL" ] || return 0
  if q_active sccache-node.service; then
    gc_l=$(q_core_limit sccache-node.service)
    if [ "$gc_l" != 0 ]; then
      echo "slot-node: kernel.core_pattern stays '$gc_v' while sccache-node.service runs with the core limit ${gc_l:-unknown}: the next apply loads $CORE_SYSCTL"
      return 0
    fi
  fi
  gc_l=$(q_core_limit_next slot-node.service)
  if [ "$gc_l" != 0 ]; then
    echo "slot-node: kernel.core_pattern stays '$gc_v' while slot-node.service starts with the core limit ${gc_l:-unknown}: the next apply loads $CORE_SYSCTL"
    return 0
  fi
  a_sysctl_load "$CORE_SYSCTL"
  gc_v=$(q_core_pattern)
  case $gc_v in
    "|"*) echo "slot-node: kernel.core_pattern is still '$gc_v' after sysctl -p $CORE_SYSCTL" ;;
    *) echo "slot-node: kernel.core_pattern is '$gc_v' (sysctl -p $CORE_SYSCTL)" ;;
  esac
}
# The bound of a drain (mode drain-bound, run by the timer of a_drain_arm). The daemon process of
# the mark still runs: the unit restarts the hard way, which ends the jobs that still run. Another
# process runs: the drain is over, and that daemon is left alone. None runs: the unit is started,
# so a daemon that drained and exited does not stay down. The lines go to the journal.
drain_bound() {
  [ -z "$T" ] || rm -f "$S/drain-timer"
  [ -f "$R$DRAIN_DIR/drain" ] || return 0
  drain_read
  if [ -n "$draining" ]; then
    db_n=$(q_jobs_left)
    echo "slot-node: the drain reached its bound (DRAIN_MAX=$DRAIN_MAX) with ${db_n:-?} job(s) left: slot-node.service restarts now, and the jobs that still run end"
    a_restart slot-node.service || echo "slot-node: systemctl restart slot-node.service failed"
  elif ! q_active slot-node.service; then
    echo "slot-node: slot-node.service is not active after its drain: starting it"
    a_start slot-node.service || echo "slot-node: systemctl start slot-node.service failed"
  fi
  rm -f "$R$DRAIN_DIR/drain" "$R$DRAIN_DIR/sccache-restart"
  return 0
}

p_services() {
  started=
  for u in slot-storage.service slot-imds-root-only.service slot-imds-containers.service docker.service slot-docker-prune.timer slot-toolchains-manifest.timer sccache-node.service slot-node.service; do
    q_active "$u" && continue
    drift "unit:$u" "not active"
    fixing || continue
    if a_start "$u"; then
      changed "unit:$u" "started"
      started="$started $u"
    else
      failed "unit:$u" "systemctl start $u"
    fi
  done
  # While slot-node.service drains nothing is signalled and nothing restarts: one line says how
  # far the drain is. A restart of sccache-node that became due meanwhile gets its mark.
  drain_read
  if [ -n "$draining" ]; then
    dr_n=$(q_jobs_left)
    dr_t=$(utc_of "$dr_until")
    drift drain "slot-node.service drains for its restart ($dr_why): ${dr_n:-?} job(s) left, restart by $dr_t"
    fixing || return 0
    if q_active sccache-node.service; then
      why_sc=$(unit_why sccache-node.service)
      if [ -n "$why_sc" ]; then
        echo "$why_sc" >"$R$DRAIN_DIR/sccache-restart"
        echo "pending: running: sccache-node restarts when slot-node does, at the end of its drain: $why_sc" >&2
      fi
    fi
    echo "draining: slot-node: ${dr_n:-?} job(s), restart by $dr_t"
    return 0
  fi
  if [ -e "$R$DRAIN_DIR/drain" ] || [ -e "$R$DRAIN_DIR/sccache-restart" ]; then
    drift drain "the mark of a drain that is over is still in $DRAIN_DIR (apply removes it)"
    if fixing; then
      rm -f "$R$DRAIN_DIR/drain" "$R$DRAIN_DIR/sccache-restart"
      a_drain_disarm
      changed drain "the mark of a drain that is over removed"
    fi
  fi
  # A running unit that is not what its file and the config say restarts when no job runs:
  # slot-node holds the build user's jobs, and a restart of sccache-node ends the compilers the
  # cache server runs for them. With slot-node not active, or started in this run, no job runs;
  # a daemon that does not say how many run counts as one that has jobs. With jobs, the restart
  # waits for a later apply, or with --drain for the end of a drain that starts now.
  why_sc=
  why_sn=
  for u in sccache-node.service slot-node.service; do
    q_active "$u" || continue
    case " $started " in *" $u "*) continue ;; esac
    why=$(unit_why "$u")
    [ -n "$why" ] || continue
    drift running "$u: $why"
    case $u in
      sccache-node.service) why_sc=$why ;;
      *) why_sn=$why ;;
    esac
  done
  fixing || return 0
  [ -n "$why_sc$why_sn" ] || return 0
  j=0
  if q_active slot-node.service; then
    case " $started " in
      *" slot-node.service "*) ;;
      *) j=$(q_jobs) ;;
    esac
  fi
  if [ "${j:-1}" = 0 ]; then
    for u in sccache-node.service slot-node.service; do
      case $u in
        sccache-node.service) why=$why_sc ;;
        *) why=$why_sn ;;
      esac
      [ -n "$why" ] || continue
      # systemctl restart starts what the manager has loaded.
      case $why in "the loaded definition"*) a_daemon_reload ;; esac
      a_restart "$u" && changed running "$u restarted: $why"
    done
    return 0
  fi
  if [ -n "$drain" ]; then
    drain_start
    return 0
  fi
  [ -z "$why_sc" ] || echo "pending: running: sccache-node restarts when no job runs ($j now): $why_sc" >&2
  [ -z "$why_sn" ] || echo "pending: running: slot-node restarts when no job runs ($j now): $why_sn" >&2
  return 0
}

# The CPU limit of the node's pool. Slots bound how many jobs run, not what they use: on a node
# of 32 CPUs and 32 slots, 14 jobs ran about 76 threads at a load of 68 to 97, and `slot
# cpu-limit` said `no CPU limit`. With a limit the daemon holds a queued job while the pool's
# jobs and the compile cache use more CPUs than the mark, or while the host's CPU pressure
# (`some avg10`) is at the pressure mark or over it. The marks: CPU_LIMIT, or 80 % of the CPUs
# found with one decimal (32 CPUs: 25.6, never under 1); CPU_PRESSURE, or 20: on a node of 32
# CPUs without a limit every CPU was busy at the lowest pressure seen, 21.8 %. CPU_LIMIT=off is
# no limit, and then no pressure mark either: the daemon keeps one only beside a CPU mark.
cpu_limit_want() {
  case $CPU_LIMIT in
    '') q_nproc | awk '{ v = $1 * 0.8; if (v < 1) v = 1; printf "%.1f\n", v }' ;;
    *) echo "$CPU_LIMIT" ;;
  esac
}
cpu_pressure_want() {
  if [ "$CPU_LIMIT" = off ]; then echo off; else echo "${CPU_PRESSURE:-20}"; fi
}
# $1 and $2 are the same mark: both off, or the same number (25.6 and 25.60, 40 and 40.0).
same_mark() {
  if [ "$1" = off ] || [ "$2" = off ]; then
    [ "$1" = "$2" ]
  else
    awk -v a="$1" -v b="$2" 'BEGIN { exit !(a + 0 == b + 0) }'
  fi
}
# The marks the daemon holds, from its answer (q_cpu_limit_answer): cl_g the CPU mark and cl_gp
# the pressure mark, each a number or `off`. A daemon from before the pressure mark answers
# without that key: the mark is off. cl_g empty: no answer, or one that is not the whole object
# with a CPU mark that is a number or null.
cpu_limit_read() {
  cl_g=
  cl_gp=
  cl_ans=$(q_cpu_limit_answer | tr -d '\n\r')
  case $cl_ans in
    "{"*"}") ;;
    *) return 0 ;;
  esac
  cl_g=$(cpu_mark cpu_busy_max "$cl_ans")
  cl_gp=$(cpu_mark cpu_pressure_max "$cl_ans")
  cl_gp=${cl_gp:-off}
  case "$cl_g$cl_gp" in *"?"*) cl_g= ;; esac
  return 0
}
# The daemon answered, and holds both marks that are wanted (cl_w, cl_wp).
cpu_limit_holds() {
  [ -n "$cl_g" ] && same_mark "$cl_g" "$cl_w" && same_mark "$cl_gp" "$cl_wp"
}
# The daemon must run: this part comes after p_services. It keeps the marks in its state
# directory on the instance store, so a node that was stopped and started gets them again here.
p_cpu_limit() {
  cl_w=$(cpu_limit_want)
  cl_wp=$(cpu_pressure_want)
  cpu_limit_read
  cpu_limit_holds && return 0
  if [ -n "$cl_g" ]; then
    drift cpu-limit "the pool holds a queued job from '$cl_g' busy CPUs and '$cl_gp' % CPU pressure, want '$cl_w' and '$cl_wp'"
  else
    drift cpu-limit "the daemon of the pool gives no answer about its CPU limit (GET $CPU_LIMIT_URL as $U, 5 s at most), want '$cl_w' and '$cl_wp'"
  fi
  fixing || return 0
  if ! q_active slot-node.service; then
    failed cpu-limit "slot-node.service is not active: the limit was not set"
    return 0
  fi
  if [ "$cl_w" = off ]; then cl_a=off; else cl_a="$cl_w --pressure $cl_wp"; fi
  cl_o=$(a_cpu_limit "$cl_w" "$cl_wp" 2>&1)
  cl_o=$(echo "$cl_o" | tail -n 1)
  # A daemon that started a moment ago takes the call on its socket before it answers the
  # request: 5 tries, 1 s apart.
  for cl_i in 1 2 3 4 5; do
    cpu_limit_read
    [ -z "$cl_g" ] || break
    [ -n "$T" ] || [ "$cl_i" = 5 ] || sleep 1
  done
  if [ -z "$cl_g" ]; then
    failed cpu-limit "after slot cpu-limit $cl_a the daemon of the pool gives no answer about its CPU limit${cl_o:+: $cl_o}"
  elif ! cpu_limit_holds; then
    failed cpu-limit "after slot cpu-limit $cl_a the pool holds a queued job from '$cl_g' busy CPUs and '$cl_gp' % CPU pressure${cl_o:+: $cl_o}"
  elif [ "$cl_w" = off ]; then
    changed cpu-limit "no CPU limit (slot cpu-limit off)"
  else
    changed cpu-limit "a queued job is held from $cl_w busy CPUs and $cl_wp % CPU pressure (slot cpu-limit $cl_a)"
  fi
}

# check --canary: the limit holds the node below the load at which its jobs stop getting done. A
# consumer's test step ran 12 to 19 s at a load of 48 to 72 on 32 CPUs, and 182 s and 258 s at 85
# to 97. The bound is a 5-minute load average of 2.5 per CPU (32 CPUs: 80). With CPU_LIMIT=off
# there is no limit to try.
p_cpu_limit_canary() {
  [ -n "$canary" ] && [ "$mode" = check ] || return 0
  [ "$(cpu_limit_want)" != off ] || return 0
  [ -z "${draining:-}" ] || return 0
  cl_l=$(q_load5)
  if ! is_decimal "$cl_l"; then
    drift cpu-limit "canary: the 5-minute load average could not be read: nothing was tested"
    return 0
  fi
  cl_n=$(q_nproc)
  cl_b=$(awk -v n="$cl_n" 'BEGIN { printf "%g\n", n * 2.5 }')
  if awk -v l="$cl_l" -v b="$cl_b" 'BEGIN { exit !(l + 0 > b + 0) }'; then
    drift cpu-limit "canary: the 5-minute load average is $cl_l on $cl_n CPUs, over $cl_b (2.5 per CPU): the limit does not hold the node"
  else
    echo "canary: ok: cpu-limit: the 5-minute load average is $cl_l on $cl_n CPUs (bound $cl_b)"
  fi
}
# check --canary while slot-node.service drains: the daemon whose jobs the two canaries above
# speak for takes no job and is about to be replaced, so neither says ok; one line names them.
p_drain_canary() {
  [ -n "$canary" ] && [ "$mode" = check ] && [ -n "$draining" ] || return 0
  dc_l=core
  [ "$(cpu_limit_want)" = off ] || dc_l="$dc_l, cpu-limit"
  echo "canary: not tested while slot-node.service drains: $dc_l"
}

# The run-time limit of the node's pool: RUN_LIMIT, after which the daemon ends a job (exit 124),
# or off, and RUN_LIMIT_MAX, the most a job names with `slot --timeout`. A daemon nobody set has
# no limit and lets a job name 1 h. Both are compared in seconds, the form of the daemon's
# answer: 10m, 600s and 600 are one limit. The daemon fixes the limit into a job when the job
# starts, so setting it ends no job that runs.
#
# run_limit_read: rl_g the limit (seconds or `off`) and rl_gm the most a job names (seconds), from
# the daemon's answer (q_run_limit_answer). rl_g empty: no answer, or one that is not the whole
# object with a limit of whole seconds or null and a maximum of whole seconds.
run_limit_read() {
  rl_g=
  rl_gm=
  rl_ans=$(q_run_limit_answer | tr -d '\n\r')
  case $rl_ans in
    "{"*"}") ;;
    *) return 0 ;;
  esac
  rl_g=$(cpu_mark run_limit_s "$rl_ans")
  rl_gm=$(cpu_mark run_limit_max_s "$rl_ans")
  case $rl_gm in '' | *[!0-9]* | ????????????????*) rl_g= ;; esac
  case $rl_g in
    off) ;;
    *[!0-9]* | ????????????????*) rl_g= ;;
  esac
  return 0
}
# The daemon answered, and has the limit and the maximum that are wanted (rl_w, rl_wm).
run_limit_holds() {
  [ -n "$rl_g" ] || return 1
  if [ "$rl_g" = off ] || [ "$rl_w" = off ]; then
    [ "$rl_g" = "$rl_w" ] || return 1
  else
    [ "$rl_g" -eq "$rl_w" ] || return 1
  fi
  [ "$rl_gm" -eq "$rl_wm" ]
}
# What the pool does, in the words of the lines below: the limit $1 and the maximum $2.
run_limit_text() {
  if [ "$1" = off ]; then
    echo "the pool has no run-time limit, and a job names up to '$2' s"
  else
    echo "the pool ends a job that runs longer than '$1' s, and a job names up to '$2' s"
  fi
}
# After p_services, like the CPU limit: the daemon must run.
p_run_limit() {
  rl_wm=$(conf_seconds "$RUN_LIMIT_MAX")
  if [ "$RUN_LIMIT" = off ]; then
    rl_w=off
    rl_wt="'off' and '$rl_wm' s ($RUN_LIMIT_MAX)"
  else
    rl_w=$(conf_seconds "$RUN_LIMIT")
    rl_wt="'$rl_w' s ($RUN_LIMIT) and '$rl_wm' s ($RUN_LIMIT_MAX)"
  fi
  run_limit_read
  run_limit_holds && return 0
  if [ -n "$rl_g" ]; then
    drift run-limit "$(run_limit_text "$rl_g" "$rl_gm"), want $rl_wt"
  else
    drift run-limit "the daemon of the pool gives no answer about its run-time limit (GET $RUN_LIMIT_URL as $U, 5 s at most), want $rl_wt"
  fi
  fixing || return 0
  if ! q_active slot-node.service; then
    failed run-limit "slot-node.service is not active: the limit was not set"
    return 0
  fi
  rl_a="$RUN_LIMIT --max $RUN_LIMIT_MAX"
  rl_o=$(a_run_limit "$RUN_LIMIT" "$RUN_LIMIT_MAX" 2>&1)
  rl_o=$(echo "$rl_o" | tail -n 1)
  for rl_i in 1 2 3 4 5; do
    run_limit_read
    [ -z "$rl_g" ] || break
    [ -n "$T" ] || [ "$rl_i" = 5 ] || sleep 1
  done
  if [ -z "$rl_g" ]; then
    failed run-limit "after slot run-limit $rl_a the daemon of the pool gives no answer about its run-time limit${rl_o:+: $rl_o}"
  elif ! run_limit_holds; then
    failed run-limit "after slot run-limit $rl_a $(run_limit_text "$rl_g" "$rl_gm")${rl_o:+: $rl_o}"
  elif [ "$rl_w" = off ]; then
    changed run-limit "no run-time limit, and a job names up to $RUN_LIMIT_MAX (slot run-limit $rl_a)"
  else
    changed run-limit "a job that runs longer than $RUN_LIMIT is ended (exit 124), and a job names up to $RUN_LIMIT_MAX (slot run-limit $rl_a)"
  fi
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
    p_core
    p_role
    p_docker
    p_slot
    [ -n "$imds_open" ] || p_rust
    [ -n "$imds_open" ] || p_tools
    p_archives
    p_make
    [ -z "$T" ] || [ -z "${NODE_TEST_MUSL_PIN:-}" ] || MUSL_PIN=$NODE_TEST_MUSL_PIN
    p_musl
    [ -z "$T" ] || [ -z "${NODE_TEST_DASH_PIN:-}" ] || DASH_PIN=$NODE_TEST_DASH_PIN
    p_dash
    [ -z "$T" ] || [ -z "${NODE_TEST_HOME_PINS:-}" ] || HOME_PINS=$NODE_TEST_HOME_PINS
    [ -n "$imds_open" ] || p_home_pins
    [ -n "$imds_open" ] || p_keys
    p_services
    p_core_load
    p_cpu_limit
    p_run_limit
    p_docker_rule
    p_canary
    p_core_canary
    p_cpu_limit_canary
    p_musl_canary
    p_firefox_canary
    p_drain_canary
    [ -n "$imds_open" ] || p_source_tools
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
  start-pre)
    # NODE_TEST=1: the whole start of the unit, as systemd does it (a_start runs the step).
    if [ -n "$T" ]; then a_start slot-node.service; else gap_step; fi
    exit 0
    ;;
  drain-bound)
    drain_bound
    exit 0
    ;;
  *) usage ;;
esac
