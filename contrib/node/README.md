# Build node provisioning

Scripts that bring a cloud machine to the state a slot build node needs, and report where a
running node differs from it. Written for Amazon Linux 2023 on an instance type with NVMe
instance storage (the `c6id` family); POSIX sh.

| file | runs | does |
|---|---|---|
| `provision.sh` | on the node, as root | storage, packages, build user, units, keys, slot release; `check` and `apply` |
| `push.sh` | on the workstation, as the build user | copies the workstation's `/usr`, some `/etc` entries, rust toolchains and cargo tools to the node; `check` and `apply` |
| `conf.sh` | sourced by both | reads the config file |
| `toolchains.sh` | on both sides | one line per toolchain and a digest of `/usr` and `/etc`, compared before a node job runs |
| `test.sh` | in `task check` | tests of `provision.sh` under a temporary root |

## The config file

Everything that names a person, an organization, a machine or a private tree is in one config
file outside the repository; `node.conf.example` shows every key with placeholder values. A
missing required key, an unknown key or a malformed value stops both scripts with exit status 2.

| key | used by | meaning |
|---|---|---|
| `BUILD_USER`, `BUILD_UID`, `BUILD_HOME` | both | the user jobs run as; `BUILD_HOME` equals the workstation's home for same-path mirrors |
| `BUILD_KEY` | provision | the workstation's build public key; it may only run `slot shell` |
| `SETUP_KEY` | provision | a second public key, let in only by `apply --open`, for `push.sh apply` |
| `SLOT_VERSION` | both | the slot release installed on the node |
| `MIRROR_DIRS` | provision | directories under `BUILD_HOME` that are bound from the instance store |
| `GO_PRIVATE` | provision | `GOPRIVATE` and `GONOSUMDB` of jobs |
| `TOKENS` | provision | slots of the node's pool; default: the CPUs found |
| `MIN_FREE_DISK`, `SCCACHE_GB`, `TMP_GB` | provision | disk floor, compile cache size, job tmp size (GB) |
| `STORE`, `DISK_MODEL`, `EXTRA_PACKAGES`, `NODE_NAME` | provision | instance store mount (default `/srv/slot`), its disk model, more packages, the name in messages |
| `NODE_SSH`, `PUSH_HOST` | push | the ssh command and the host of the setup login |
| `NODE_HOST`, `NODE_RUN_DIR` | push | the node's name for `SLOT_HOST` and a checkout to run the check job from |
| `RUST_TOOLCHAINS`, `HOME_COPIES`, `SKIP_BINS` | push | toolchains to copy (default: all), directories under the home to copy, cargo tools to leave out |

## A new node, or a node after a stop and start

The instance store is empty after every stop and start; the root volume keeps the rest.

1. As root: `provision.sh apply CONF --open`. On a node that ran it before, `slot-storage.service`
   has already rebuilt the array, the job tmp filesystem and the binds at boot.
2. On the workstation: `push.sh apply CONF`.
3. As root: `provision.sh apply CONF` closes the setup login and starts `sccache-node` and
   `slot-node`.
4. `provision.sh check CONF` (root) and `push.sh check CONF` (workstation) both print `0 drift`.

A root channel that takes one script, such as SSM Run Command, gets it from
`provision.sh bundle CONF apply` (or `check`): one script with the config inside.

## What check compares

`provision.sh check` reads and prints; it changes nothing. `apply` never restarts a running
`slot-node` while jobs run or queue, never changes an existing user, never formats a device
that holds a filesystem, and as root it neither writes nor mounts through a symbolic link in
the build user's directories. Files it replaces keep their old version beside them as
`<name>.slot-prev`.
