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
| `role.sh` | where the node's owner has the AWS CLI | the node's own IAM role and instance profile, and the instance's metadata options; `plan`, `check` and `apply` |
| `conf.sh` | sourced by both | reads the config file |
| `toolchains.sh` | on both sides | one line per rust toolchain, the go and node versions and the version of each C tool of the machine it runs on |
| `toolchains-refresh.sh` | on the workstation, as a user unit | every 60 s: writes this host's `toolchains.sh` lines and fetches the node's into `~/.local/state/slot` |
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
| `SLOT_TARGET` | provision | which archive of that release the node installs: `x86_64-unknown-linux-gnu` (the default, the build against glibc) or `x86_64-unknown-linux-musl` (the static build); `slot --version` is the same for both, so `check` reads the build from the binary at each path and from the running daemon |
| `MIRROR_DIRS` | provision | directories under `BUILD_HOME` that are bound from the instance store |
| `GO_PRIVATE` | provision | `GOPRIVATE` and `GONOSUMDB` of jobs |
| `TOKENS` | provision | slots of the node's pool; default: the CPUs found |
| `MIN_FREE_DISK`, `SCCACHE_GB`, `TMP_GB` | provision | disk floor, compile cache size, job tmp size (GB) |
| `DOCKER_GB` | provision | what docker may store on the root disk before the prune timer removes what no job uses (GB, default 10, at least 1) |
| `DOCKER_CONTAINERD_STORE` | provision | `on`: dockerd keeps its images in containerd's image store, so that `docker buildx build --output type=docker,dest=<file>` writes a tar with the default builder (`node.conf.example` has `on`); `off` or absent: `/etc/docker/daemon.json` and the store are left as they are. Images of the old store are not visible after the switch (What check compares, below) |
| `CPU_LIMIT`, `CPU_PRESSURE` | provision | the CPU limit of the node's pool: the busy CPUs from which a queued job is held (from 1, one decimal; default 80 % of the CPUs found; `off` for no limit) and the percent of CPU pressure from which it is held as well (over 0, at most 100; default 20; `off`) |
| `RUN_LIMIT`, `RUN_LIMIT_MAX` | provision | the run-time limit of the node's pool: how long a job runs before the daemon ends it (a duration: seconds, or a number with `s`, `m` or `h`; default `10m`; `off` for no limit) and the most a job names with `slot --timeout` (default `30m`; not shorter than `RUN_LIMIT`) |
| `DRAIN_MAX` | provision | the bound of a drain started by `apply --drain`: after it the unit restarts with the jobs that still run (a duration from `1m` to `24h`, default `30m`) |
| `GO_ROOT` | provision | the directory the go archive is unpacked into, for a node whose `go env GOROOT` must answer the path the workstation's go answers (`/usr/lib/go`); empty: `/usr/local/go-<version>` |
| `STORE`, `DISK_MODEL`, `NODE_NAME` | provision | instance store mount (default `/srv/slot`), its disk model, the name in messages and in the node's instance profile (`slot-node-<NODE_NAME>`) |
| `INSTANCE_PROFILE` | provision | the instance profile jobs may have docker under, when it is not `slot-node-<NODE_NAME>`; `none` for a node outside EC2 |
| `EXTRA_PACKAGES` | provision | dnf packages besides the base set: JDKs, maven, llvm, database and browser libraries and the like (`node.conf.example`) |
| `EXTRA_TOOLS` | provision | tools installed from pins only on a node that names them: `erlang`, `elixir` (only beside `erlang`), `gleam`, `odin`, `firefox` (with the packages it loads); a name with no pin is refused; empty: none of them is installed or checked. `erlang` is built from source and needs `ncurses-devel` in `EXTRA_PACKAGES` |
| `CLANG_VERSION` | provision | the major release of the `clang` on a job's PATH (`20`): the packages `clang<major>` and `clang<major>-libs`, and `clang`, `clang++` and `clang-cpp` in `/usr/local/bin` as links to `/usr/bin/clang-<major>` and the like; digits only; empty: the distribution's `clang` is left alone. Taken out of the config, the links stay as they are |
| `RUST_TOOLCHAINS` | provision | exact rustup toolchain names, space separated (`1.99.1 stable`); empty: rust is left alone |
| `RUST_COMPONENTS`, `RUST_TARGETS` | provision | components (default `clippy rustfmt`) and targets (default none): a bare name goes to every toolchain, `<toolchain>:<name>,<name>` to that toolchain only (`1.97:x86_64-unknown-linux-musl`, `1.99.1:rust-src`); a prefix must be a word of `RUST_TOOLCHAINS`; `apply` removes from a toolchain every non-host target, and every clippy, rustfmt, rust-src or llvm-tools component, that no entry names for it (with `RUST_TARGETS` empty, all non-host targets); `check` names each one and compares the lists with the workstation's per toolchain |
| `NODE_SSH`, `NODE_HOST`, `NODE_RUN_DIR` | push | the ssh command, the node's name for `SLOT_HOST` and a checkout to run the check job from |
| `PUSH_HOST` | none | read by no script now; accepted so that an older config file still loads |

## A new node, or a node after a stop and start

The instance store is empty after every stop and start; the root volume keeps the rest.

1. Launch the image: Amazon Linux 2023 on an instance type with instance storage. Where the AWS
   CLI has an identity that may write IAM: `role.sh apply NAME INSTANCE` gives the instance a
   role of its own (The node's role, below). Under another role the build user gets no docker.
2. As root: `provision.sh apply CONF`. It installs the packages, the build user, the units, the
   slot release, rustup (release 1.29.3, checked against its SHA-256) with every toolchain of
   `RUST_TOOLCHAINS`, and the pinned tools, and starts `sccache-node` and `slot-node`. On a node
   that ran it before, `slot-storage.service` has already rebuilt the array, the job tmp
   filesystem and the binds at boot, and `apply` installs again what lived on the instance store.
3. `provision.sh check CONF --canary` (root; outside EC2 without `--canary`) and `push.sh check
   CONF` (workstation) both print `0 drift`.

A root channel that takes one script, such as SSM Run Command, gets it from
`provision.sh bundle CONF apply` (or `check`, or `check --canary`): one script with the config
inside. `bundle CONF apply` takes `--open` and `--drain` as `apply` does.

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

## The node's role

The metadata service gives the credentials of the instance's role to root on the node, and a job
that may use docker can be root there (a container on the host network). So a node has a role of
its own that is worth nothing to a job, and jobs get docker only under it.

`role.sh` makes it, where the AWS CLI has an identity that may write IAM and change an instance.
Nothing of it runs on the node, and `provision.sh` calls no AWS API.

| command | does |
|---|---|
| `role.sh policy`, `role.sh trust` | print the policy document and the role's trust document |
| `role.sh plan NAME INSTANCE` | prints the `aws` commands of a first setup and calls nothing |
| `role.sh check NAME INSTANCE` | reads the account: one `drift: role:` line per difference, exit 1 |
| `role.sh apply NAME INSTANCE` | makes what `check` names, then checks again; two differences it names and leaves (below) |

`NAME` is the config's `NODE_NAME`, `INSTANCE` the instance id. What `apply` leaves:

- The customer-managed policy `slot-node-agent`, shared by every node: 11 actions with which the
  Systems Manager agent stays a managed node and takes Run Command and Session Manager.
  `ssm:UpdateInstanceInformation` is its heartbeat. The four `ssmmessages:*Channel` actions carry
  commands and sessions (agent release 3.3.40 and later). The six `ec2messages` actions carry
  them for an older agent.
- The role and the instance profile `slot-node-<NAME>`, with that one policy. A second attached
  policy comes off after the node's policy is on. An inline policy of the role and another role
  in the profile stay: `apply` prints a `failed: role:` line with the command and exits 1.
- The instance uses that profile. Credentials the former role gave out stay valid until they
  expire; the agent takes the new role at its next refresh or at
  `systemctl restart amazon-ssm-agent`.
- The instance's metadata service wants a token and answers over one hop (`HttpTokens=required`,
  hop limit 1).

Left out of what the managed policy `AmazonSSMManagedInstanceCore` allows: `ssm:GetParameter`
and `ssm:GetParameters` (they read every parameter of the account), `ssm:GetDocument` and
`ssm:DescribeDocument`, and the State Manager, inventory, patch and distributor actions. A node
has no association; the agent logs a refused `ListInstanceAssociations`.

Not in the policy either: `kms:Decrypt` for Session Manager sessions encrypted with a KMS key,
and the S3 and CloudWatch Logs writes for session logs or Run Command output sent there. An
account whose Session Manager preferences set those adds them to the document in `role.sh`.

Who holds the role's credentials can act as this instance's agent until they expire: read the
commands sent to it and answer them. A job with docker is root on the node and can do that
without them.

On the node, `provision.sh` asks the metadata service three things: which instance profile the
instance has, which role it gives out, and what it does with a request that carries no token.

- The build user gets group `docker` when the profile is `slot-node-<NODE_NAME>` and gives out
  the role of that name, or when the instance has no profile; and the service refuses a request
  without a token (status 401 or 403).
- `INSTANCE_PROFILE=<name>` names another profile as the node's own. That is the owner's word:
  the role in it is not compared, and nothing on the node reads what the role allows.
- `INSTANCE_PROFILE=none` is for a machine outside EC2 (its DMI system vendor is not
  `Amazon EC2`): the metadata service is not asked. On an EC2 instance it accepts no profile.
- Anything else is `drift: role: ...` in `check`; `apply` prints `failed: role: ...`, exits 1
  and adds no group: another profile, another role in the node's profile, a service that
  answers without a token, and every question left without an answer. Every other part of the
  node is made all the same.
- `apply` does not take the group away from a build user that has it. The failed line says so
  and names the command. A `slot-node` that started before the group is not restarted for it
  while the role part fails.

What the node cannot see: a second policy attached to the role `slot-node-<NAME>`. `role.sh
check` reads that, where the owner's identity is.

`provision.sh check CONF --canary` also runs `role-canary.sh` (installed under
`/usr/local/libexec/slot-node/`): one container as root on the host network, from the curl
project's image named by digest, takes the role's credentials from the metadata service and
asks Systems Manager for a parameter. A refusal (`AccessDenied`) is the line `canary: ok: ...`,
and so is an instance without a role. An allowed call, a container that does not run and a run
that could test nothing (no token, no credentials, no running docker) are
`drift: role: canary: ...`. The first run pulls the image. A refusal is about that one call:
a role that reads parameters under one path only, or reads another service, passes it.
`--canary` also times an aborted process, reads the load average, compiles one C file with
`musl-gcc -static` and, with `firefox` in `EXTRA_TOOLS`, starts Firefox headless (What check
compares, below).

## What check compares

`provision.sh check` reads and prints; it changes nothing. `apply` never restarts a running
`slot-node` while jobs run or queue (`apply --drain` starts a drain, whose bound does: Draining,
below), never changes an existing user, never formats a device
that holds a filesystem, and as root it neither writes nor mounts through a symbolic link in
the build user's directories. Files it replaces keep their old version beside them as
`<name>.slot-prev`.

The node uses its own `/usr` and `/etc`. Everything a job needs beyond the base packages comes
from `EXTRA_PACKAGES`, `RUST_TOOLCHAINS` or a pin.

Packages that Amazon Linux 2023 lacks are not installed; a job that needs one is held by name:
mingw cross compilers, `mawk`, `erlang`, `elixir`, `odin` and `gh`. The config file does
not list them.

Of these, `erlang`, `elixir` and `odin` are held only on a node whose config does not name them
in `EXTRA_TOOLS`: a node that names them there gets them, and `gleam`, from pins (below).

No credential goes to a node. `STORE/nodebin` is the first directory of a job's PATH, and each
script in it prints one line on stderr and exits 255: `no-credentials` (the `GIT_ASKPASS` and
`SSH_ASKPASS` of `slot-node.service`), `ssh` (it lets through only `-G` and `-V`, which open no
connection), and `glab` and `gh`, which work only with a credential (`NO_CREDENTIAL_TOOLS` in
`provision.sh`). A task that calls `glab api` on the node ends with that line, not with what its
next command makes of an empty answer. The line names the way out: run the step on the
workstation with `slot --here <command>`, or name the command in `.slot.toml` `local = [...]` of
its repository.

Rust: rustup is installed for the build user into `STORE/rustup` and `STORE/cargo` (`~/.rustup`
and `~/.cargo` link there) from the pinned release of `rustup-init`, with no default toolchain and
no change to the shell profile. Each name in `RUST_TOOLCHAINS` is installed with the minimal
profile plus `RUST_COMPONENTS` and `RUST_TARGETS`. `check` names a missing rustup (`rustup`) and
each missing toolchain (`rust:<name>`); a toolchain that is there is not installed again, so
`stable` is not moved by `apply`. A toolchain that is there still gets the components and targets
it lacks: `check` names each one (`rust:<name>: component rustfmt missing`) and `apply` runs
`rustup component add` or `rustup target add` for those only. A failed download is a `failed:`
line and exit status 1.

The default toolchain is `stable` when `RUST_TOOLCHAINS` has it, as on the workstation: without
it rustup makes the first toolchain it installs the default (1.85.0), and a job that names none
(`cargo` with no `+<toolchain>` and no `rust-toolchain.toml`) ran that one. `check` reads
`default_toolchain` in `STORE/rustup/settings.toml` and names a difference
(`rust:default: the default toolchain is '…', want stable-x86_64-unknown-linux-gnu`); `apply` runs
`rustup default stable` (a first run on a node reads the rustup output for what it did).

The metadata service is closed first: `apply` starts `slot-imds-root-only.service` and reads the
firewall back before it installs a key, rustup, a pinned tool or anything else the build user
runs. A start that does not close it is a `failed: imds:` line, and those parts wait for the next
`apply`. Commands that run as the build user get a clean environment (`HOME`, the units' `PATH`,
`LANG`, the rust homes for rustup) and, where `setpriv` can, a session of their own.

docker is there for the jobs that start containers (a consumer's OCI lane): the distribution's
`docker` package (a base package), `docker.service` enabled and started, and the build user in
group `docker`. `slot-node.service` gets the group when it starts, so on a node that ran before
it restarts when no job runs (`pending: running: ... started before <user> was in group docker`
until then).

A container's packets to the metadata service are forwarded and pass the rule of
`slot-imds-root-only.service`. `slot-imds-containers.service` rejects them in the `DOCKER-USER`
chain, a drop-in makes `docker.service` require that unit, and `check` reads the rules back from
the firewall. What this does not close:

- A member of group `docker` is root on the node: a privileged container, a host-network
  container, an ipvlan or macvlan network.
- dockerd fetches as root on the host: `ADD http://169.254.169.254/...` in a Dockerfile and
  `docker import <url>` pass both rules.

So the instance requires IMDSv2 (a plain GET gets no credential), and its role carries nothing a
job may not use: The node's role, above.

docker stores on the root disk (`/var/lib/docker`). `slot-docker-prune.timer` runs
`docker-prune.sh` every 10 minutes:

- With no job in the pool, running or queued, it removes every container, running ones too: a
  job that ended left it. Nothing else on the node may keep a container.
- While docker holds more than `DOCKER_GB`: containers that do not run and were created over an
  hour ago, and the unused build cache. With no job in the pool also every image and every volume
  no container uses.
- While the pool has a job, images and volumes stay, also one a job pulled, built or loaded and
  has not started yet. docker may then hold more than the cap; the unit fails with one line that
  says how much. The cap is checked every 10 minutes and is not a quota.

The node holds no registry credential, so a job pulls public images only.

`DOCKER_CONTAINERD_STORE=on` turns on containerd's image store. With the classic store the
default buildx builder (driver `docker`) refuses an image written to a file: `ERROR: Docker
exporter is not supported for the docker driver. Switch to a different driver, or turn on the
containerd image store, and try again.` (buildx 0.12.2, BuildKit 0.12.6). A CI runner's docker
builds the same command. With containerd's store the default builder writes the tar, and `docker
build` still loads its image into docker as before (a `docker-container` default builder would
not). It needs Docker Engine 24.0 or later; Amazon Linux 2023 packages docker 25.0 (25.0.16 and
containerd 2.3.5 in release 2023.12).

- `check` names `drift: docker-store: /etc/docker/daemon.json does not turn on the containerd
  image store ("features": {"containerd-snapshotter": true})`, and `drift: running:
  docker.service: docker info names the image store 'overlay2', not containerd's` while dockerd
  runs the classic store (`docker info` as root: its `DriverStatus` names the driver type
  `io.containerd.snapshotter.v1` with containerd's store).
- `apply` merges that key into the object `features` of `/etc/docker/daemon.json` and keeps
  every other key (the file before is `daemon.json.slot-prev`). The node may have no `jq`: the
  text is merged with `awk`, and `dockerd --validate` checks the result before it is written.
  A file that is no JSON object is left alone (`failed: docker-store: ...`). A second `apply`
  changes nothing.
- dockerd reads the file when it starts, and its restart ends the containers of the jobs: it
  waits for them like a restart of `slot-node`. With no job in the pool `apply` restarts docker
  (`changed: running: docker.service restarted: ...`); with jobs it prints `pending: running:
  docker restarts when no job runs (<n> now): ...`, and `apply --drain` makes it happen in the
  gap before `slot-node` starts again (the mark `docker-restart`, Draining below). On a bare node
  the file is written before docker first starts, and nothing restarts.
- Images, containers and volumes of the old store are not visible after the switch, and a job
  pulls or builds its images again. Their files stay under `/var/lib/docker` and count toward
  `DOCKER_GB` until they are removed: `docker system prune -af --volumes` before the switch (with
  no job in the pool) removes them; the prune timer does not see them afterwards.

A running unit follows its file. `sccache-node.service` and `slot-node.service` restart when
`apply` rewrote the unit's file or finds its process started from another definition, and
`slot-node.service` also when its `slot serve` arguments or the slot binary changed. Both wait for
an empty pool: a restart of `slot-node` ends the jobs, and one of `sccache-node` ends the
compilers the cache server runs for them. With a job running `apply` prints
`pending: running: ...`, and an `apply` that finds no job and still finds the difference restarts
the unit. `check` names a running unit whose loaded definition or process environment is not the
unit file's, one whose soft core limit is not 0, and a `slot-node` that runs another slot version
than `SLOT_VERSION`.

`SLOT_TARGET` names which archive of the release is installed:
`slot-<SLOT_VERSION>-x86_64-unknown-linux-gnu.tar.gz`, the default and the one a node has
installed so far, or `slot-<SLOT_VERSION>-x86_64-unknown-linux-musl.tar.gz`, the static build,
which needs no glibc of the node. Either is checked against the line of its own name in the
release's `SHA256SUMS`, so a release without the musl archive installs nothing (`failed: slot:
download or checksum ...`). Both builds print the same `slot --version`, so a change of the key
at one version is read from the binary: the build against glibc names a program interpreter and
the static one none (`readelf -l`, from binutils, which the base package `gcc` requires; where
`readelf` gives no answer, the build is not compared). `check` prints `drift: slot:<path>: is the
build for <target>, config says <target>` for each of the three paths and `drift: running:
slot-node.service: it runs the slot build for ...` for a daemon that still runs the other one;
`apply` installs the archive of the key (`changed: slot: slot <version> (x86_64-unknown-linux-musl)
installed at ...`) and restarts `slot-node` like after a version change: when no job runs, or
through `apply --drain`. One node takes the musl build first, by `SLOT_TARGET` in its own config
file; the default changes only after that node ran it under real jobs.

Draining: a node that always has a job never gets that restart (one node: 10 runs of `apply` in
29 minutes, 5 to 14 jobs each time). `apply --drain` (also `bundle CONF apply --drain`) starts it
without an empty pool. With a restart of either unit pending and jobs in the pool, it reloads the
manager, arms a bound and sends SIGTERM to the daemon process alone
(`systemctl kill --kill-whom=main`), prints `draining: slot-node: <n> job(s), restart by <UTC
time>` and goes on to its end; the exit status is the one without the drain. The daemon removes
its socket, lets the jobs it has run and queued end, and exits; no job is signalled. systemd then
starts the unit again (`Restart=always`), with the unit file and the build user's groups of that
moment. While it drains the node takes no new job: a call from a workstation waits for the
daemon (`SLOT_NODE_WAIT`). With no job in the pool `apply --drain` restarts at once, as `apply`
does, and without `--drain` nothing changes.

- The gap: `slot-node.service` runs `provision.sh start-pre` as root before every start
  (`ExecStartPre`), whoever started it. A restart of `sccache-node.service` that was pending
  happens there, with no daemon of the pool running (`pending: running: sccache-node restarts
  when slot-node does, at the end of its drain: ...` until then), and the marks of the drain go.
  A pending restart of `sccache-node` alone drains `slot-node` too: the gap is its only moment.
  A pending restart of `docker.service` (`DOCKER_CONTAINERD_STORE`) waits for the gap the same
  way (`pending: running: docker restarts when slot-node does, at the end of its drain: ...`).
  At the end of a drain the step also loads the core pattern (`sysctl -p
  /etc/sysctl.d/60-slot-core.conf`) when `apply` would: the pattern is a pipe, the file is there,
  `sccache-node` runs with the soft core limit 0 and `slot-node.service` starts with it
  (`systemctl show -p LimitCORESoft`). So one `apply --drain` is enough; the journal of
  `slot-node.service` has the line `slot-node: kernel.core_pattern is 'core' (sysctl -p ...)`,
  or one that says which unit keeps the pipe. A start that ends no drain loads nothing.
- The bound: `DRAIN_MAX` after the signal, a transient timer (`slot-node-drain.timer`) runs
  `provision.sh drain-bound`. If the daemon process that got the signal still runs (same main PID
  and start time), the unit restarts the hard way, which ends the jobs that still run, and the
  journal of `slot-node-drain.service` says so. Another daemon process is left alone. A unit that
  is not active is started. A bound that cannot be armed means no signal (`failed: drain: ...`).
- During a drain `check` has one line for it, `drift: drain: slot-node.service drains for its
  restart (<why>): <n> job(s) left, restart by <UTC time>`, in place of the `running` lines, and
  `check --canary` prints `canary: not tested while slot-node.service drains: core, cpu-limit`
  and neither `ok` line. `apply`, with `--drain` or without, prints the `draining:` line again
  and sends no second signal. The jobs left are read from the daemon's `/metrics`.
- The marks are files of root in `/run/slot-node` (`drain`: the daemon process, the time of the
  bound, the reason; `sccache-restart`; `docker-restart`). After the restart nothing of the drain is left; a mark
  that outlived its drain is one `drift: drain:` line, and `apply` removes it.

Core dumps: a bare node has `kernel.core_pattern` as a pipe to `systemd-coredump`, and the kernel
runs a pipe handler whatever the core limit of the process is. A process killed by SIGABRT was
reaped only after the handler: 142 to 256 ms later on an idle node, up to 3.5 s with 24 at once,
and tests that wait 5 to 7 s for an aborted child timed out under load. The node's pattern is the
file name `core`, from `/etc/sysctl.d/60-slot-core.conf` (`kernel.core_pattern=core`; the name
sorts after systemd's `50-coredump.conf`), and `slot-node.service` and `sccache-node.service` have
`LimitCORE=0:infinity`: a job that aborts, or a compiler the cache server runs for it, writes no
core file and is reaped at once. The limit is the soft one. A job that wants a core file raises
it (`ulimit -c unlimited`) and gets the file in the working directory of the process that
crashed: `core`, or `core.<pid>`, one per crash, where `kernel.core_uses_pid` is 1.

Under the pattern `core` a process whose limit is not 0 writes such a file, so `apply` loads the
pattern only when both units run with the soft limit 0. A unit that runs with another limit
restarts when no job runs, and the same `apply` then loads the pattern. With a job running,
`apply` writes the unit files and the sysctl file, prints `pending: running: ...` for each unit
and `pending: core: ...`, and the pipe stays until an `apply` finds no job, or, after `apply
--drain`, until the start of `slot-node.service` that ends the drain, which loads the pattern
under the same condition (Draining, the gap). `check` names a
pattern that is a pipe and a running unit whose soft core limit is not 0. A pattern that is
neither a pipe nor `core` is the owner's choice and stays: with no `60-slot-core.conf` there,
`apply` writes none, and `check` and `apply` say so in a `kept: core: ...` line; a
`60-slot-core.conf` that is already there stays, and it makes the pattern `core` at the next
boot. With the pattern `core`, `coredumpctl` gets no dump from the node. A process outside those
two units whose core limit is not 0 writes a core file as well: a process in a container a job
starts does, into its working directory in the container, which is a directory of the node when
the job mounted its tree there, unless the job passes `--ulimit core=0`. `check --canary` aborts
a process five times under the soft core limit 0 and reads the quickest time until it is reaped,
and reads the soft core limit of the running `slot-node`: over 50 ms, a limit that is not 0 or a
`slot-node` that is not active is `drift: core: canary: ...`, else the line
`canary: ok: core: ...`.

The CPU limit of the pool: slots bound how many jobs run, not what they use. On a node of 32 CPUs
and 32 slots, 14 jobs ran about 76 threads at a load of 68 to 97, with a CPU pressure
(`some avg10` of `/proc/pressure/cpu`) of 35.67 at load 68. Once `slot-node.service` runs, `apply`
sets `slot cpu-limit <CPU_LIMIT> --pressure <CPU_PRESSURE>` as the build user: the daemon holds a
queued job while the pool's jobs and the compile cache use more CPUs than the first mark, or while
the host's CPU pressure is at the second mark or over it (4 jobs always run, and a job held for
10 minutes starts). The defaults are 80 % of the CPUs found (32 CPUs: 25.6) and 20;
`CPU_LIMIT=off` is no limit. The pressure default is from 15 rounds within 7 minutes on one node
of 32 CPUs (16 cores) without a CPU limit, with a fixed CPU workload run 8 wide. Every round had
31.7 to 32.0 of 32 CPUs busy. In 8 rounds at a pressure of 21.8 to 43.5 % (7 to 11 jobs, load 53
to 58) the workload took 455 to 539 ms, median 490 ms; in 5 rounds at 65.0 to 76.5 % (12 to 14
jobs, load 72 to 78) it took 701 to 2186 ms, median 981 ms. Every CPU was busy at the lowest
pressure seen (21.8 %), so the default mark is 20; nothing was measured below 21.8 % or with a
free CPU. The daemon keeps the marks in its state directory, on the instance
store, so `apply` sets them again after a stop and start. `check` asks the daemon which marks it
holds (`GET /api/cpu-limit` on the loopback address, as the build user, 5 s at most) and compares
them as numbers; root opens no file of the build user for it. A daemon that gives no answer is
`drift: cpu-limit: the daemon of the pool gives no answer ...`. While `slot-node.service` is not
active, `apply` prints `failed: cpu-limit: ...`. `check --canary` reads the 5-minute load
average: over 2.5 per CPU (32 CPUs: 80) is `drift: cpu-limit: canary: ...`, else the line
`canary: ok: cpu-limit: ...`; with `CPU_LIMIT=off` it has no line. The bound is from a test step
on 32 CPUs that ran 12 to 19 s at a load of 48 to 72, and 182 s and 258 s at a load of 85 to 97.

The run-time limit of the pool: a daemon nobody set has none and lets a job name up to 1 h with
`slot --timeout`; jobs ran 23 minutes on a node. Once `slot-node.service` runs, `apply` sets
`slot run-limit <RUN_LIMIT> --max <RUN_LIMIT_MAX>` as the build user (defaults `10m` and `30m`;
`RUN_LIMIT=off` is no limit): the daemon ends a job that runs longer than the limit (exit 124),
and a job that names its time gets at most the maximum. Setting the limit ends no job that runs:
the daemon fixes the limit into a job when the job starts, so a job that started before keeps
what it started with. `check` asks the daemon (`GET /api/run-limit` on the loopback address, as
the build user, 5 s at most), which answers in seconds, and compares seconds: `10m`, `600s` and
`600` are one limit. Another limit or maximum is `drift: run-limit: the pool ends a job that
runs longer than '<n>' s, and a job names up to '<n>' s, want ...`, a daemon that gives no
answer `drift: run-limit: the daemon of the pool gives no answer ...`, and `apply` with
`slot-node.service` not active prints `failed: run-limit: ...`. The daemon keeps the limit in
its state directory, on the instance store, so `apply` sets it again after a stop and start.

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
version of `go` and `node`; a missing link or another version is drift. `typescript` (`tsc`,
`tsserver`), `prettier` and `esbuild` are archives of the same kind: the npm tarballs of those
releases from `registry.npmjs.org` (esbuild's is its `@esbuild/linux-x64` package, the native
binary), with `bin/prettier.cjs` linked as `prettier`; the consumers' test lanes call them by
name. `pnpm` 11.9.0 is the same kind (`pnpm` and `pnpx` link to its `bin/*.mjs`; it needs node
22.13 or later), and `yarn` 1.22.22, the release the workstation has in `/usr/bin` (`yarn` and
`yarnpkg` link to its `bin/yarn.js`, which needs `node` on PATH; a consumer's `task install`
calls `yarn`). yarn 1 runs the script that `yarn-path` of a `.yarnrc` names and prints that
script's version, so `check` and `toolchains.sh` read `yarn --version` with `YARN_IGNORE_PATH=1`:
the release that is installed answers. `toolchains.sh` prints one `tool <name> <version>` line
for each tool it finds (the list is below), and `push.sh check` compares those lines both ways.
`zstd` and `python3` are base packages. An older directory of a former pin stays until someone
removes it. Other releases are not installed: a consumer fetches its own (`GOTOOLCHAIN=auto`).
The pins: go 1.27.1 and node v22.23.3 (the `tar.gz`, not the `tar.xz`).

Firefox is an archive a node installs only when its config names it in `EXTRA_TOOLS` (below):
`firefox` there selects its line of `EXTRA_ARCHIVE_PINS` (the same form as `ARCHIVE_PINS`), and
a name that has no pin is refused before anything runs. A consumer's browser tests start `firefox` by
name, headless. The pin is Firefox 159.0, the release the workstation runs: Mozilla's archive
`linux-x86_64/en-US/firefox-159.0.tar.xz` from `archive.mozilla.org`, its SHA-256 the line of
that file in the release's `SHA256SUMS`. `apply` unpacks it into `/usr/local/firefox-159.0`
and links `/usr/local/bin/firefox`; `check` reads `firefox --version` (`Mozilla Firefox 159.0`)
with `MOZ_HEADLESS=1`. The libraries the browser loads beside its own come from the packages
`gtk3`, `alsa-lib`, `libX11-xcb` and `dbus-libs` (and what `gtk3` requires), and `xz` unpacks
the archive: `EXTRA_TOOL_PACKAGES` adds them to the packages of a node that names firefox.
`check --canary` starts it as the build user in a new directory of `STORE/tmp` (its `HOME`,
`TMPDIR` and profile), `--headless --no-remote --remote-debugging-port 0`, waits up to 30 s for
`WebDriver BiDi listening on ws://...` on standard error, ends it and removes the directory:
`canary: ok: firefox: ...`, or `drift: firefox: canary: ...` with the first line Firefox printed
when it ended before that line or did not print it. On a node, from a job, the start reached
that line after 406 ms. `toolchains.sh` prints `tool firefox <release>`.

`GO_ROOT`: go takes its GOROOT from the real path of its binary, so on a node `go env GOROOT`
answers `/usr/local/go-<version>`, and on a workstation with a distribution's go `/usr/lib/go`.
A consumer whose output carries that path builds on the node only when both answer the same one.
A link at that path would not do: go resolves it and answers the versioned directory. With
`GO_ROOT=/usr/lib/go` the go archive is unpacked there as a real directory, `go` and `gofmt` in
`/usr/local/bin` link into it, and `check` compares what `go env GOROOT` answers the build user
with `GO_ROOT` (`drift: archive:go: go env GOROOT of ... answers '<path>', want <GO_ROOT>`).
`toolchains.sh` prints the answer on both sides as a line `goroot <path>`, after the `go` line,
which is unchanged; `push.sh check` does not compare it.

- The path is one others may own. The directory carries a mark, the file `.slot-archive` with
  `go <version> <sha256 of the archive>`. A `GO_ROOT` that exists without the mark, that is a
  symbolic link, or that a package owns (`rpm -qf`) is not written to: `check` has one drift
  line and `apply` one `failed: archive:go: GO_ROOT <path> ...: nothing was changed` line that
  names the owner or the package. A path with a symbolic link in it is not written through.
- The config refuses a `GO_ROOT` that is not a plain absolute path, and one that is a directory
  the provisioning manages, lies in one or holds one: `STORE`, `BUILD_HOME`, `/usr/local/bin`,
  `/usr/local/libexec`, `/usr/bin`, `/etc`, `/run`, and the directory of a pinned archive.
- The move: `apply` writes the new directory, switches the links, and removes the old
  `/usr/local/go-<version>` when the pool counts no job (a go build that runs reads its
  GOROOT). With a job running it prints `pending: archive:go: /usr/local/go-<version> is removed
  when no job runs`, and a later `apply` removes it. `check` prints the same `pending:` line for
  the leftover, on stdout; it is no drift, so the check still ends with `0 drift`.
- A release change replaces the directory whole, as it does without `GO_ROOT`. A `GO_ROOT` taken
  out of the config, or changed, leaves its old directory: nothing names it, so the owner
  removes it.

`ess` 0.99.2 and `cargo-about` 0.9.1 are pinned like `task`, in the build user's cargo bin: `ess` is
the release the workstation has on PATH. It is a dispatcher: in a tree whose `ess-inputs.yaml`
requires another release it runs that release, which it downloads itself (SHA-256 checked) into
the build user's `~/.cache/ess/toolchains`, so a node job needs `github.com` over https and a
writable `~/.cache`. `yq` is the Go one of mikefarah at v4.30.8, an archive pin (the tarball names
its binary `yq_linux_amd64`, linked as `yq`). `toolchains.sh` prints a `tool` line for `ess`,
`aep`, `yq` and `cargo-about` as well, read from `/` with `ESS_TOOLCHAIN` unset, and one each for
`pnpm` and `yarn`.

`helm` v3.8.1 is an archive pin too: the static binary of `get.helm.sh`, the release the
workstation runs, linked as `/usr/local/bin/helm` (a consumer's chart check runs `helm lint` and
`helm template`). helm has no `--version`, so `check` and the `tool helm` line of `toolchains.sh`
read `helm version --template {{.Version}}`. Run with the prefix `""` (the workstation and the node
both), `toolchains.sh` reads the helm its PATH names first, the one a job started there runs, before
`/usr/local/bin`, `/usr/bin` and `~/.local/bin`: a workstation can have a second helm of another
release in `/usr/bin`. A helm that a relative PATH entry names is read by its absolute path. This is the one `tool` line that
depends on the caller's PATH: run `push.sh check` from a shell with the PATH a job gets, or the
line names the other helm. The `pnpm` line needs `node` 22.13 or later on the caller's PATH;
without it the line reads `tool pnpm missing`. The `yarn` line needs `node` there too.

`aep` 0.80.0 on the PATH of a job is built on the node (the release binary is installed too, for
runbook's driver cache path only, see below: it wants glibc 2.39 and the node has 2.34). A
source pin (`SOURCE_PINS` in `provision.sh`) names the repository, the git revision of the release
tag (the pin: there is no archive to hash) and the rust toolchain; apply runs `cargo +1.99.1
install --locked --git <url> --rev <rev> aep-cli --root <store>/cargo` as the build user after
the rust part, and check reads `aep --version`. The build needs `github.com` over https (aep and
its `entity-runtime` dependency are read without credentials). `push.sh check` exits 1 whenever the
workstation has a tool whose release the node's pin does not match, `aep` included. Packages
`glibc-static` (a consumer's `cc -static` tests), `bubblewrap`, `socat`, `unzip` and `libubsan` (the
undefined-behaviour sanitizer runtime of a consumer's sanitizer build) are base packages, and
`/usr/sbin` is in the job PATH of `slot-node.service` (`ip`). The workstation's `cargo` and `go`
shims send the real binary by absolute path and the node runs that argv as sent, so `/usr/bin/cargo`,
`cargo-clippy`, `cargo-fmt`, `clippy-driver`, `rustc`, `rustdoc`, `rustfmt`, `rustup` link to the
build user's rustup proxies and `/usr/bin/go`, `gofmt` to `/usr/local/bin`; a real file at one of
those names is left alone and reported as `failed`.

GNU Make 4.4.1 is built on the node (`MAKE_PIN` in `provision.sh`): the node's make 4.3 cannot
parse the `--jobserver-auth=fifo:` form that the daemon puts in `MAKEFLAGS`, so a cmake or make
build read the pool as no jobserver and ran with `-j` unbounded or serial. Apply downloads the
tarball from `ftp.gnu.org`, checks its SHA-256, runs `./configure --disable-nls --without-guile`
and `make` as the build user, installs the binary in `/usr/local/make-4.4.1/bin` and links it as
`make` and `gmake` in `/usr/local/bin`; check reads `make --version`. A link that alone drifts is
set again without a download, and a real file at either name is left alone and reported as
`failed`. The build runs `/usr/bin/make` under `timeout 600`. `/usr/bin/make` (4.3) stays;
`/usr/local/bin` is before `/usr/bin` in the job PATH, so only a job that names `/usr/bin/make` by
path still gets 4.3.

`musl-gcc` is built on the node from the source of musl 1.2.6 (`MUSL_PIN` in `provision.sh`), the
release the workstation has: Amazon Linux 2023 packages none, and a build that names it as the C
compiler of the musl target (`CC_x86_64_unknown_linux_musl=musl-gcc`) stopped with `failed to find
tool "musl-gcc"`. Apply downloads the tarball from `musl.libc.org`, checks its SHA-256 before
anything is unpacked, and the build user runs `./configure --prefix=/usr/local/musl-1.2.6
--syslibdir=/usr/local/musl-1.2.6/lib --enable-wrapper=gcc`, `make -j4` and `make install` into a
directory of the source tree, under `timeout 600`. Root copies that tree to
`/usr/local/musl-1.2.6` and links `bin/musl-gcc`, the wrapper that runs the node's `gcc` with
`lib/musl-gcc.specs`, as `/usr/local/bin/musl-gcc`. The build runs once: `check` reads the link,
the release that `lib/libc.so` names and whether `musl-gcc --version` answers (`drift: musl:
...`), a link that alone drifts is set again without a download, a tree of another release is
replaced whole, and a real file at the link's name is left alone and reported as `failed`. On a
node of 32 CPUs at a load of 58 to 68 the build took 31 s of `make -j4` (104 s with one job), 1 s
of `configure` and 3 to 8 s of `make install`. `check --canary` writes one C file that includes
`stdio.h`, compiles it with `musl-gcc -static` as the build user in a new directory of
`STORE/tmp`, runs the result and removes the directory: `canary: ok: musl: ...`, or `drift: musl:
canary: ...` with the compiler's first error line. `toolchains.sh` prints `tool musl <release>`,
read from the loader beside the wrapper's specs file on both machines, so `push.sh check` names
another release; its `c musl-gcc` line reads the wrapper in `/usr/local/bin` too. A change of the
pin's release builds a new directory and moves the link; the directory of the release before
stays, like the one of make.

`dash` is built on the node from the source of dash 0.5.12 (`DASH_PIN` in `provision.sh`), the
release that is `sh` on the forge's runner image (`ubuntu-26.04`, package `dash 0.5.12-12ubuntu3`):
Amazon Linux 2023 packages none, a node's `sh` is bash, and a shell test that passes under bash can
fail under dash (a redirection of a command is applied by dash in the shell itself, so `readlink
/proc/$$/fd/1 >file` reads the file's name there and the name of the shell's output under bash).
Apply downloads upstream's release tarball, checks its SHA-256 before anything is unpacked (the
site serves plain http only; the SHA-256 is also the one of the `dash_0.5.12.orig.tar.gz` of
Ubuntu's and Debian's package pools), and the build user runs `./configure` and `/usr/bin/make`
under `timeout 600`. Root installs the one binary in `/usr/local/dash-0.5.12/bin`, writes the
tarball's SHA-256 beside it (`/usr/local/dash-0.5.12/.slot-sha256`) and links it as
`/usr/local/bin/dash`. It is never linked as `sh`: a node's `sh` stays the distribution's, and a
job that wants the forge's shell names `dash`. dash prints no version, so `check` reads the link,
the SHA-256 file and the answer of `dash -c 'echo $((6 * 7))'` (`drift: dash: ...`); a link that
alone drifts is set again without a download, and a real file at the link's name is left alone
and reported as `failed`. On a node of 32 CPUs the download took 1.7 s, `configure` 1.8 s and
`make` with one job 3.2 s. `toolchains.sh` prints `tool dash <release>`: on a node the release of
the directory the link names (`missing` when that tree has no SHA-256 file), elsewhere the release
the machine's package manager names for its package `dash` (pacman, dpkg or rpm, without the
package revision), and no line on a machine without dash. So `push.sh check` names a workstation
that has no dash, or another release, as drift. The gate of this repository uses it:
`contrib/shell-tests.sh` runs the shell tests under dash where dash is on PATH (a directory with
`sh` linked to dash goes first on PATH), under `sh` elsewhere, and its first line says which.

`HOME_PINS` in `provision.sh` lists trees that jobs read by path in the build user's home. The ess
releases 0.97.4, 0.98.0, 0.99.1 and 0.99.4 are in `~/.local/share/ess-versions/<version>/ess`
(a repository that pins an ess release calls that path; the SHA-256 is the one of the release's
`SHA256SUMS`), and `@types/node` 26.3.0 with `undici-types` 8.3.0 is in
`~/.local/share/ess-node/node_modules`, the directory `ESS_TYPES_NODE` names. Apply downloads each
archive that is missing or of another version, checks its SHA-256, and the build user copies the
tree into place through a `.slot-new` directory, with group and other write and setuid removed, and
writes the archive's SHA-256 to `.slot-sha256` in the tree. Check runs `ess --version` or reads
`package.json`, and reports drift when `.slot-sha256` is not the pin's SHA-256. These steps are
skipped while the metadata service is open, like the other downloads that run as the build user.

The same list holds the aep release binary of runbook's driver pin
(`~/.cache/sdc/aep/<version>/x86_64-unknown-linux-gnu/aep`; the driver compares its SHA-256 with
`driver/aep-pin.json`, so the aep built from source on the node's PATH cannot stand in for it). The
release binary asks for glibc 2.39 through two weak symbols; on the node's 2.34 the loader prints
one warning and `aep --version` still answers. `toolchains.sh` prints
`tool sdc-aep-<version> <16 hex digits of the SHA-256>` for it.

The same list holds the Playwright browsers a repository's tests use: the Chrome for Testing
headless shell of revision 1254 (Playwright 1.65.0) and 1234 (1.62.1) in
`~/.cache/ms-playwright/chromium_headless_shell-<revision>`. Their lines have the strip field `zip`:
the archive is unzipped without stripping, the files `INSTALLATION_COMPLETE` and
`DEPENDENCIES_VALIDATED` are written, and the check is the version `--version` prints. `playwright
install` does not know Amazon Linux, so the browser is not installed that way. The libraries it
loads (`nss`, `atk`, `at-spi2-atk`, `mesa-libgbm`, `libxkbcommon`, `gtk3`) come from
`EXTRA_PACKAGES`; `unzip` is a base package. A new Playwright version is a new line: the revision
is in the `browsers.json` of `playwright-core`, the version in its `browserVersion`.

The same list holds Groovy 2.4.10, the binary distribution of Maven Central, at the path sdkman
uses (`~/.sdkman/candidates/groovy/2.4.10`): a consumer's tasks read
`embeddable/groovy-all-2.4.10.jar` and `bin/groovyc` there unless `GROOVY_HOME` names another
directory. Its line has the strip field `zip1` (a zip with one top directory, which is stripped;
an archive with anything else at the top is refused) and a check that ends in `.jar`: the file
exists and is no symbolic link. `toolchains.sh` prints
`tool groovy-all-<version> <16 hex digits of the jar's SHA-256>` for each version directory under
that path that has the jar, so `push.sh check` reports a workstation without that candidate, or
with another 2.4 release, as drift. The JDK comes from `EXTRA_PACKAGES`.
sdkman's `current` link is made too: `~/.sdkman/candidates/groovy/current` names the version of
the first groovy line (`2.4.10`, relative, as sdkman makes it), so a job that carries the
workstation's `GROOVY_HOME=~/.sdkman/candidates/groovy/current` (named in `SLOT_ENV`) finds
the tree. The build user writes it after the trees; a missing link, or one to another version,
is `drift: home:groovy-current: ...`, and a directory or file at that path is left alone
(`failed: ... is no symbolic link: not replaced`). `JAVA_HOME` gets no such link: a job that
carries the workstation's `~/.sdkman/candidates/java/current` should not name it in `SLOT_ENV`;
a node's `java` is `/usr/bin/java`, and its JDKs are under `/usr/lib/jvm`.

`EXTRA_TOOLS` names tools that only some consumers need, space separated: `erlang`, `elixir`,
`gleam`, `odin` and `firefox` (a consumer's tasks call `gleam`, `elixir`, `mix`, `erl` and `odin`
by name, its browser tests start `firefox`, and Amazon Linux 2023 packages none of them). A node whose config has no `EXTRA_TOOLS` gets none of
them and nothing of them is checked; a tool taken out of the key stays as it is. A name without a
pin is refused, and so is `elixir` without `erlang`. Each tool is pinned in `provision.sh` by
version and SHA-256, the download is checked before anything of it is unpacked, and each binary
is a link in `/usr/local/bin`, which is on the PATH of every job:

- `erlang`: Erlang/OTP 29.0.6 (`ERLANG_PIN`), built on the node from the source tarball of its
  release; the SHA-256 is the one in the release's `SHA256.txt`. Upstream publishes no Linux
  binary, and the builds of the Hex build site for Ubuntu did not serve on a node (glibc 2.34,
  OpenSSL 3.5): the one for 22.04 starts and cannot load its crypto library (`undefined symbol:
  EVP_sm4_cbc`), so `mix` fetches no package; the one for 24.04 asks for `GLIBC_2.38`. The build
  is like the one of musl: the build user runs `./configure --prefix=/usr/local/erlang-29.0.6
  --without-javac --without-odbc --without-wx --without-debugger --without-observer --without-et`,
  `make -j4` and `make install` into a directory of the source tree, under `timeout 1800`, and
  root copies that tree to `/usr/local/erlang-29.0.6`. `erl`, `erlc` and `escript` are links to
  `lib/erlang/bin` of that tree. The build needs `gcc`, `gcc-c++`, `make`, `perl` and
  `openssl-devel`, which are base packages, and `ncurses-devel`, which the node names in
  `EXTRA_PACKAGES`: without it `check` says `drift: erlang: the build needs the package(s)
  ncurses-devel, not installed: name them in EXTRA_PACKAGES`, and `apply` fetches nothing and
  reports `failed`. A build that leaves no crypto library is refused. The build runs once: the
  tree carries the mark `.slot-archive` (`erlang 29.0.6 <sha256 of the tarball>`), and `check`
  reads the three links, the mark and the OTP release that `erl` answers (`drift: erlang: ...`).
  Links that alone drift are set again without a download, a tree with another mark or release
  is replaced whole, and a real file at a link's name is left alone and reported as `failed`. On
  a node of 32 CPUs at a load of 10 to 21 the build took 11 to 12 s of `configure`, 79 s of
  `make -j4` (58 s with 8 jobs) and 6 s of `make install`; the tree has 152 MiB, and the source
  tree with its objects, in a temporary directory that apply removes, 724 MiB.
- `elixir`: Elixir 1.20.4, the precompiled release for OTP 29 (`elixir-otp-29.zip`; the SHA-256
  is the one of the release's `elixir-otp-29.zip.sha256sum`), in
  `/usr/local/elixir-1.20.4-otp-29`, with `elixir`, `elixirc`, `mix` and `iex` linked. Its check
  reads `Elixir 1.20.4 (compiled with Erlang/OTP 29)` from `elixir --version`, which runs `erl`:
  an apply that could not install Erlang/OTP leaves elixir as it is and says so
  (`failed: archive:elixir: left as it is: ...`).
- `gleam`: Gleam 1.19.0, the static binary of its release (the SHA-256 is the one of the
  release's `.sha256` file), in `/usr/local/gleam-1.19.0`.
- `odin`: Odin dev-2026-09, the release archive for linux-amd64 with its `core`, `base` and
  `vendor` libraries beside the binary, in `/usr/local/odin-dev-2026-09`. The release publishes
  no checksum file: the SHA-256 is the digest of the archive as downloaded from the release on
  two machines, equal to the digest in the release's asset list. `odin` links with the `clang`
  on PATH: the base package's, or the major `CLANG_VERSION` names (below). Its check reads the
  release from `odin version`
  (`dev-2026-09-nightly:<commit>`).
- `firefox`: Firefox 159.0 in `/usr/local/firefox-159.0`, with the packages it loads, and the
  start that `check --canary` makes (above).

`elixir`, `gleam`, `odin` and `firefox` are archive pins like go and node
(`EXTRA_ARCHIVE_PINS`, drift lines `archive:elixir`, `archive:gleam`, `archive:odin`,
`archive:firefox`). `toolchains.sh` prints one `tool` line for each of the other four on a
machine that has it (firefox's line is above): `tool erlang 29.0.6` (the OTP release, which `erl`
reads from its `releases/<major>/OTP_VERSION`), `tool elixir 1.20.4-otp-29`, `tool gleam 1.19.0`
and `tool odin dev-2026-09-nightly:a2fb372`, so `push.sh check` names a release that differs
between the workstation and the node, and a tool only one side has. Like the `helm` line, these
four depend on the caller's PATH on a workstation, where the one PATH names first is read:
compare them from a shell with the PATH a job gets. The `elixir` line is `missing` where no
`erl` is on PATH.

`CLANG_VERSION` sets the major release of the `clang` a job finds on PATH. Amazon Linux 2023's
`clang` package is clang 15 (`/usr/bin/clang`, a base package); the distribution also packages
newer majors as `clang20`, `clang21` and `clang22`, each installing `/usr/bin/clang-<major>`,
`/usr/bin/clang++-<major>` and `/usr/bin/clang-cpp-<major>` and requiring
`clang<major>-libs`. A consumer that reads clang's JSON syntax tree
(`clang -Xclang -ast-dump=json -fsyntax-only`) from PATH fails with clang 15's. With
`CLANG_VERSION=22` the node gets the packages `clang22` and `clang22-libs` with the other
packages, and `clang`, `clang++` and `clang-cpp` in `/usr/local/bin`, which a job's PATH has
before `/usr/bin`, are links to `/usr/bin/clang-22`, `/usr/bin/clang++-22` and
`/usr/bin/clang-cpp-22`. `/usr/bin/clang` stays the distribution's. `check` names a missing link
or one to another binary (`drift: clang: /usr/local/bin/clang++ is '/usr/bin/clang', want a link
to /usr/bin/clang++-22`) and a `clang --version`, run as the build user from `/usr/local/bin`,
whose first line does not name `clang version 22.` (`drift: clang: clang --version as the build
user (/usr/local/bin/clang) says '...', want clang 22`). `apply` sets the links that drift, only
to a binary that is there (`failed: clang: /usr/bin/clang-22 is missing (package clang22): not
linked` otherwise), leaves a real file at a link's name alone and reports it as `failed`, and
reports a `clang` of another major after the links as `failed`. Another value moves the links to
that major; the packages of the major before stay. Taken out of the config (or left empty), the
key changes nothing: the links stay as they are, nothing of them is checked, and removing them
is a step by hand.

Read from a node's package repositories on 2026-11-26: `clang21` and `clang22` cannot be
installed beside the base package `clang` (clang 15). Their `clang<major>-libs` require
`clang<major>-resource-filesystem`, which conflicts with `clang-resource-filesystem < 21.99`
(`< 22.99`), and clang 15's `clang-libs`, `compiler-rt` and `libomp-devel` require that package
(and `clang`, `clang-devel` and `clang-tools-extra` require `clang-libs`). `clang20` installs
beside clang 15, and a consumer's C reader tests that failed with clang 15's syntax tree passed with clang 20.1.8 (166 passed, 0 failed), so the example config names `20`. `apply` installs the clang packages in a dnf transaction of their own, so such a
conflict fails them alone, names dnf's conflict line (`failed: packages: dnf install clang22
clang22-libs (CLANG_VERSION): package clang22-resource-filesystem-... conflicts with
clang-resource-filesystem < 22.99 ...`), and leaves the other packages installed; no link is set
to a binary that is not there. `provision.sh` does not remove the base clang.

`push.sh check` runs `toolchains.sh` on the workstation and in one slot job on the node and
compares the `rust` lines (toolchain name, rustc commit, digest of its targets and components),
the `go` line (`go go1.27.1`: the bare version, so the workstation's `go1.27.1-X:nodwarf5` and
the node's `go1.27.1` are equal), the `node` line (`node v22.23.3`) and the `tool` lines (both
ways: a tool one side has and the other lacks is drift), and the release of each pinned tool on the node
(`pin <name> <version>`; another release or a missing one is drift). The C tools of
`toolchains.sh` (`c <tool> <version>`) are printed but not compared: the workstation and the node
are different distributions. `push.sh check` prints one line saying that nothing is copied.
It is a version parity report, not an integrity check: the job runs as the build user, and a job
that runs as that user can alter what the node reports. Control characters in the node's output
are dropped before anything is printed.
`slot-toolchains-manifest.timer` writes the node's `toolchains.sh` output to a file under the
build user's `.local/state/slot` every 60 s.

`toolchains-refresh.sh NODE [SECONDS]` keeps both manifests on the workstation for a wrapper that
compares them before it sends a job: `toolchains-local.txt` (what `toolchains.sh` beside the
script prints) and `toolchains-node.txt` (the node's file, fetched by `rsync` with the ssh
config `~/.config/slot/node/ssh_config` from the same path under the same home;
`TOOLCHAINS_NODE_FILE` names another path) in `~/.local/state/slot`. Each is written through a
temporary file of the run's process and a rename. A `toolchains.sh` that prints nothing, a node
that does not answer within 60 s, and an empty file or one over 1 MiB keep the manifest that is
there, with one line on stderr that names the reason. `SECONDS` 0 writes both once: exit 0 when
both were written. The two files have one name each, so one state directory serves one node: the
loop holds a lock (`toolchains-refresh.lock`, its text the node), and a second loop or a single
run for another node exits 2.

As a service: copy `toolchains-refresh.sh` and `toolchains.sh` to `~/.local/share/slot/` and
`contrib/slot-toolchains-refresh@.service` to `~/.config/systemd/user/`. The local manifest names
the tools the script's PATH finds first, and a user unit does not get the PATH of a login shell
(on one host `helm` was v3.8.1 from the shell and v4.3.0 from the user manager's PATH). So write
the PATH of the shell that starts builds into the unit's environment file, from that shell:
`printf 'PATH=%s\n' "$PATH" >~/.config/slot/node/toolchains-refresh.env`. Then
`systemctl --user enable --now slot-toolchains-refresh@NODE`. The unit starts the script again
10 s after it ends, except after exit 2 (a usage error or the lock).
