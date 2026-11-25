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
| `ETC_EXTRA` | both | more `/etc` entries jobs read from the workstation (a JDK links its config there) |
| `RUST_TOOLCHAINS`, `HOME_COPIES`, `SKIP_BINS` | push | toolchains to copy (default: all), directories under the home to copy, cargo tools to leave out |

## A new node, or a node after a stop and start

The instance store is empty after every stop and start; the root volume keeps the rest.

1. As root: `provision.sh apply CONF --open`. On a node that ran it before, `slot-storage.service`
   has already rebuilt the array, the job tmp filesystem and the binds at boot.
2. On the workstation: `push.sh apply CONF`.
3. As root: `provision.sh apply CONF` closes the setup login and starts `sccache-node` and
   `slot-node`.
4. `provision.sh check CONF` (root) and `push.sh check CONF` (workstation) both print `0 drift`.

A running `slot-node` keeps the `/etc` files it bound at its start; after a `push.sh apply`
that replaced one, restart it when no job runs.

Removing the setup key from `authorized_keys` does not end an ssh connection that is already
logged in, and a control connection (`ControlMaster`, `ControlPersist`) shared through the ssh
config is reused by every later call. `push.sh apply` keeps its own control connection and ends
it on exit; give the setup host no `ControlMaster` in the ssh config. `provision.sh apply`
without `--open` also ends every login of this boot made with the setup key that still runs (sshd
logs each connection's pid and key fingerprint): with pam_systemd it ends the login's logind
session, which takes every process the login started, also one whose parent became pid 1;
`check` reports one as drift.

`provision.sh apply` writes `HOME_COPIES` to `/etc/slot-node/copies`. `push.sh apply` reads its
config once, prints `push: copy list: …`, and stops with exit status 2 when its list differs from
the node's: a list changed after the root step that approved it needs that root step again.

## Choosing `HOME_COPIES`

A tool that a job reads from a directory under a `MIRROR_DIRS` entry, and that is not in
`HOME_COPIES`, is gone after a stop and start. Ask every session that builds on the node which
paths under the home outside its repositories its gates read, and add them in one change.

- List a cache directory file by file before adding it: a cache holds whatever was downloaded
  into it, not only what its name says.
- Leave out anything that came from a repository or origin the remote deny lists name, even
  when a gate reads it; the gate then names the exact files it needs, or runs on the
  workstation.
- Leave out credentials, the agent's own configuration, and caches a build fills again by
  itself.

A root channel that takes one script, such as SSM Run Command, gets it from
`provision.sh bundle CONF apply` (or `check`): one script with the config inside.

## A change of instance size

A stop and start in another instance type of the same family, as run on 2026-11-24 from 64 to 32
CPUs. The node's slots follow the CPUs found, so the config file does not change.

1. Drain. On the workstation, set the node's `deadline` in `cluster.toml` to a time in the past
   and turn off anything that sends every build to the node (an environment file that sets
   `SLOT_HOST`); keep copies of both files. `systemctl --user reload slot`. Wait until the
   node's `slot status` reads `0 running, 0 queued`.
2. Save the node's job log: the files under the build user's `.local/state/slot` live on the
   instance store and are lost at the stop. Copy them out through a node job's output.
3. Both checks print `0 drift`. If a `push.sh apply` replaced an `/etc` file, restart
   `slot-node` first (it is idle after step 1).
4. Build the post-start scripts before the stop: `provision.sh bundle CONF apply --open`,
   `bundle CONF apply` and `bundle CONF check`, from the commit that will run, with their
   sha256 sums.
5. At the provider: stop, change the type, start. The host keys are on the root volume, so only
   the address changes; put it in the ssh config. A poweroff scheduled on the node lived in
   `/run` and is gone.
6. Run the steps of § A new node. The first `push.sh apply` copies everything again (4,582 s on
   2026-11-24).
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

A few cargo tools are not copied from the workstation: the node installs them at the release its
consumers' CI installs (`TOOL_PINS` in `provision.sh`, listed by `provision.sh pins`), from the
release archive checked against its SHA-256. `check` reads the first line of each one's
`--version`. A consumer that moves its CI to a new release needs a new pin, and a bundle with
it. `geckodriver` is pinned at the release of the forge runner image that browser test lanes run
on; the browser itself is the workstation's Firefox, copied with `/usr`. Known difference, read
2026-11-25: the workstation and the node run Firefox 159.0, the forge image (ubuntu-24.04,
20261116.376.1) Firefox 160.0 with Geckodriver 0.37.1. A lane that passes on the forge and fails
on the node on a browser feature points here first.

`ess` is never pinned: it chooses the release for each repository itself. An exact `requires: ess
X.Y.Z` in the repository's `ess-inputs.yaml` runs that release from ess's own cache (downloaded
from the ess release and checked against its `SHA256SUMS`); a line pin (`0.98`) or none runs the
base binary. The node's base `ess` is the workstation's, copied by `push.sh`, and `push.sh check`
compares its SHA-256, so both machines choose the same release for every repository. A repository
that needs another release than the base pins it exactly.

`push.sh check` compares the toolchain digest of `/usr` and `/etc`, the size of each cargo tool
but the pinned ones and `SKIP_BINS` (`push.sh bins` lists the rest), the release of each pinned
tool on the node (`pin <name> <version>`; another release or a missing one is drift),
and each file under the `HOME_COPIES` directories by name and size. A file only on the node under
one of those directories is not drift: `push.sh apply` copies without deleting, and a node job
may add files to a copied cache.

`HOME_COPIES` directories under a `MIRROR_DIRS` entry live on the instance store: after a stop and
start they are back only after `push.sh apply`. A tool a job reads from such a directory and
that is not in `HOME_COPIES` is gone after a start.
