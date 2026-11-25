# Build node provisioning

Scripts that bring a cloud machine to the state a slot build node needs, and report where a
running node differs from it. Written for Amazon Linux 2023 on an instance type with NVMe
instance storage (the `c6id` family); POSIX sh.

A node starts as a bare Amazon Linux 2023 image. `provision.sh apply` makes it complete: nothing
is copied from the workstation to a node ahead of jobs.

| file | runs | does |
|---|---|---|
| `provision.sh` | on the node, as root | storage, packages, build user, units, keys, slot release, rustup and rust toolchains, pinned tools and archives (go, node); `check` and `apply` |
| `push.sh` | on the workstation, as the build user | `check`: compares the workstation's rust, go and node versions and the pinned tools' releases with the node's; copies nothing |
| `conf.sh` | sourced by both | reads the config file |
| `toolchains.sh` | on both sides | one line per rust toolchain, the go and node versions and the version of each C tool of the machine it runs on |
| `test.sh` | in `task check` | tests of `provision.sh`, `push.sh` and `toolchains.sh` under a temporary root |

## The config file

Everything that names a person, an organization, a machine or a private tree is in one config
file outside the repository; `node.conf.example` shows every key with placeholder values. A
missing required key, an unknown key or a malformed value stops both scripts with exit status 2.

| key | used by | meaning |
|---|---|---|
| `BUILD_USER`, `BUILD_UID`, `BUILD_HOME` | both | the user jobs run as; `BUILD_HOME` equals the workstation's home for same-path mirrors |
| `BUILD_KEY` | provision | the workstation's build public key; it may only run `slot shell` |
| `SETUP_KEY` | provision | a second public key, let in only by `apply --open`, for a login that sets the node up by hand |
| `SLOT_VERSION` | both | the slot release installed on the node |
| `MIRROR_DIRS` | provision | directories under `BUILD_HOME` that are bound from the instance store |
| `GO_PRIVATE` | provision | `GOPRIVATE` and `GONOSUMDB` of jobs |
| `TOKENS` | provision | slots of the node's pool; default: the CPUs found |
| `MIN_FREE_DISK`, `SCCACHE_GB`, `TMP_GB` | provision | disk floor, compile cache size, job tmp size (GB) |
| `STORE`, `DISK_MODEL`, `NODE_NAME` | provision | instance store mount (default `/srv/slot`), its disk model, the name in messages |
| `EXTRA_PACKAGES` | provision | dnf packages besides the base set: JDKs, maven, llvm, database and browser libraries and the like (`node.conf.example`) |
| `RUST_TOOLCHAINS` | provision | exact rustup toolchain names, space separated (`1.99.1 stable`); empty: rust is left alone |
| `RUST_COMPONENTS`, `RUST_TARGETS` | provision | components (default `clippy rustfmt`) and targets (default none): a bare name goes to every toolchain, `<toolchain>:<name>,<name>` to that toolchain only (`1.97:x86_64-unknown-linux-musl`, `1.99.1:rust-src`); `check` compares them with the workstation's per toolchain |
| `NODE_SSH`, `NODE_HOST`, `NODE_RUN_DIR` | push | the ssh command, the node's name for `SLOT_HOST` and a checkout to run the check job from |
| `PUSH_HOST` | none | read by no script now; accepted so that an older config file still loads |

## A new node, or a node after a stop and start

The instance store is empty after every stop and start; the root volume keeps the rest.

1. Launch the image: Amazon Linux 2023 on an instance type with instance storage.
2. As root: `provision.sh apply CONF`. It installs the packages, the build user, the units, the
   slot release, rustup (release 1.29.3, checked against its SHA-256) with every toolchain of
   `RUST_TOOLCHAINS`, and the pinned tools, and starts `sccache-node` and `slot-node`. On a node
   that ran it before, `slot-storage.service` has already rebuilt the array, the job tmp
   filesystem and the binds at boot, and `apply` installs again what lived on the instance store.
3. `provision.sh check CONF` (root) and `push.sh check CONF` (workstation) both print `0 drift`.

A root channel that takes one script, such as SSM Run Command, gets it from
`provision.sh bundle CONF apply` (or `check`): one script with the config inside.

`apply --open` also lets `SETUP_KEY` into `authorized_keys`. Removing the setup key does not end
an ssh connection that is already logged in, and a control connection (`ControlMaster`,
`ControlPersist`) shared through the ssh config is reused by every later call; give the setup
host no `ControlMaster` in the ssh config. `provision.sh apply` without `--open` also ends every
login of this boot made with the setup key that still runs (sshd logs each connection's pid and
key fingerprint): with pam_systemd it ends the login's logind session, which takes every process
the login started, also one whose parent became pid 1; `check` reports one as drift.

## A change of instance size

A stop and start in another instance type of the same family, as run on 2026-11-24 from 64 to 32
CPUs. The node's slots follow the CPUs found, so the config file does not change.

1. Drain. On the workstation, set the node's `deadline` in `cluster.toml` to a time in the past
   and turn off anything that sends every build to the node (an environment file that sets
   `SLOT_HOST`); keep copies of both files. `systemctl --user reload slot`. Wait until the
   node's `slot status` reads `0 running, 0 queued`.
2. Save the node's job log: the files under the build user's `.local/state/slot` live on the
   instance store and are lost at the stop. Copy them out through a node job's output.
3. Both checks print `0 drift`.
4. Build the post-start scripts before the stop: `provision.sh bundle CONF apply` and
   `bundle CONF check`, from the commit that will run, with their sha256 sums.
5. At the provider: stop, change the type, start. The host keys are on the root volume, so only
   the address changes; put it in the ssh config. A poweroff scheduled on the node lived in
   `/run` and is gone.
6. Run `provision.sh apply CONF` (the bundle). Provisioning rebuilds after the stop what lived on
   the instance store: the rust toolchains, the pinned tools and the slot binary in the store;
   nothing is copied from the workstation.
7. A test job with `SLOT_HOST` set to the node: `slot status` there shows the new slot count, and
   one build of a repository exits 0.
8. Switch back: restore both files from step 1, with the new `price_per_hour_cents` from the
   provider's price list, and `systemctl --user reload slot`.
9. Schedule the poweroff at the lease end again, if the node has one: logind `ScheduleShutdown`
   with `poweroff` and the time in microseconds, which is the call `shutdown -P` makes.

## What check compares

`provision.sh check` reads and prints; it changes nothing. `apply` never restarts a running
`slot-node` while jobs run or queue, never changes an existing user, never formats a device
that holds a filesystem, and as root it neither writes nor mounts through a symbolic link in
the build user's directories. Files it replaces keep their old version beside them as
`<name>.slot-prev`.

The node uses its own `/usr` and `/etc`. Everything a job needs beyond the base packages comes
from `EXTRA_PACKAGES`, `RUST_TOOLCHAINS` or a pin.

Packages that Amazon Linux 2023 lacks are not installed; a job that needs one is held by name:
mingw cross compilers, `musl-gcc`, `dash`, `mawk`, `erlang`, `elixir`, `odin` and `gh`. The config
file does not list them.

Rust: rustup is installed for the build user into `STORE/rustup` and `STORE/cargo` (`~/.rustup`
and `~/.cargo` link there) from the pinned release of `rustup-init`, with no default toolchain and
no change to the shell profile. Each name in `RUST_TOOLCHAINS` is installed with the minimal
profile plus `RUST_COMPONENTS` and `RUST_TARGETS`. `check` names a missing rustup (`rustup`) and
each missing toolchain (`rust:<name>`); a toolchain that is there is not installed again, so
`stable` is not moved by `apply`. A toolchain that is there still gets the components and targets
it lacks: `check` names each one (`rust:<name>: component rustfmt missing`) and `apply` runs
`rustup component add` or `rustup target add` for those only. A failed download is a `failed:`
line and exit status 1.

The metadata service is closed first: `apply` starts `slot-imds-root-only.service` and reads the
firewall back before it installs a key, rustup, a pinned tool or anything else the build user
runs. A start that does not close it is a `failed: imds:` line, and those parts wait for the next
`apply`. Commands that run as the build user get a clean environment (`HOME`, the units' `PATH`,
`LANG`, the rust homes for rustup) and, where `setpriv` can, a session of their own.

A running unit follows its file. When `apply` rewrites `sccache-node.service` it restarts that
unit; `slot-node.service` restarts when its file, its `slot serve` arguments or the slot binary
changed and no job runs, else `apply` prints `pending: running: ...` and the next `apply`
restarts it. `check` names a running unit whose loaded definition or process environment is not
the unit file's, and a `slot-node` that runs another slot version than `SLOT_VERSION`.

A few tools are installed from a release archive checked against its SHA-256 (`TOOL_PINS` in
`provision.sh`, listed by `provision.sh pins`) into the store's cargo bin: the release its
consumers' CI installs, and the compile cache `sccache`, which `sccache-node.service` runs from
there. `check` reads the first line of each one's `--version`. A consumer that moves its CI to a
new release needs a new pin, and a bundle with it. `geckodriver` is pinned at the release of the
forge runner image that browser test lanes run on; the browser is the node's own package, so its
release can differ from the forge image's, and a lane that passes on the forge and fails on the
node on a browser feature points there first.

`cargo-nextest` and `task` (go-task) are pinned the same way, at the release and the SHA-256 that
consumers pin for the release asset. Their archives hold the binary at the top; `task --version`
prints the bare version, which `check` reads as `task <version>`.

Go and node are archives (`ARCHIVE_PINS` in `provision.sh`), because the distribution's packages
are older than the workstation's and a job must resolve the same toolchain on both sides. `apply`
downloads each archive from its official site (go from `go.dev/dl`, node from `nodejs.org/dist`),
checks its SHA-256 with `sha256sum -c --strict`, unpacks it as root into `/usr/local/<name>-<version>`
(group and others cannot write there, so the build user cannot replace it) and links `go`,
`gofmt`, `node`, `npm`, `npx` and `corepack` into `/usr/local/bin`. `check` reads each link and the
version of `go` and `node`; a missing link or another version is drift. An older directory of a
former pin stays until someone removes it. Other releases are not installed: a consumer fetches its
own (`GOTOOLCHAIN=auto`). The pins: go 1.27.1 and node v22.23.3 (the `tar.gz`, not the `tar.xz`).

`ess` is not pinned and not installed by these scripts: it chooses the release for each
repository itself, from its own digest-checked cache.

`push.sh check` runs `toolchains.sh` on the workstation and in one slot job on the node and
compares the `rust` lines (toolchain name, rustc commit, digest of its targets and components),
the `go` line (`go go1.27.1`: the bare version, so the workstation's `go1.27.1-X:nodwarf5` and
the node's `go1.27.1` are equal) and the `node` line (`node v22.23.3`), and the release of each pinned tool on the node
(`pin <name> <version>`; another release or a missing one is drift). The C tools of
`toolchains.sh` (`c <tool> <version>`) are printed but not compared: the workstation and the node
are different distributions. `push.sh check` prints one line saying that nothing is copied.
It is a version parity report, not an integrity check: the job runs as the build user, and a job
that runs as that user can alter what the node reports. Control characters in the node's output
are dropped before anything is printed.
`slot-toolchains-manifest.timer` writes the node's `toolchains.sh` output to a file under the
build user's `.local/state/slot` every 60 s.
