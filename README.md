# slot

One shared pool of build slots for every build on a machine.

When several coding agents (or people) build on one workstation, each `cargo test` sizes itself
to the whole machine and twelve of them run at once on twenty cores. `slot` puts every build
through one daemon that owns a GNU-make jobserver: cargo, rustc and make draw their parallelism
from the same token pool, a go job holds a fixed share, and anything else holds one slot. A job
that has to wait is queued, and the web view shows every job live.

```console
slot cargo test -p foo     # queued on the shared pool; output and exit code as usual
slot go test ./...         # holds 4 slots, runs with -p 4
slot task check            # wrappers pass the pool down to cargo inside
slot --local cargo build   # bypass the daemon
```

## Install

```console
cargo install --git https://github.com/beyond10x/slot --locked b10x-slot-cli
slot serve                 # or install the systemd user unit: contrib/slot.service
```

Open <http://127.0.0.1:7411/> for the live view.

## Remote build server

Run `slot serve` on the server. On the workstation:

```console
export SLOT_HOST=buildbox
slot cargo test -p foo
```

`slot` rsyncs the repository (without `.gitignore`d files, `.git` and `target/`) to the server
and talks to the daemon through `ssh buildbox slot proxy`. Target directories stay on the server.
If the server is unreachable the command runs locally and `slot` prints one line saying so.

## Documentation

<https://beyond10x.github.io/slot/>: getting started, agent integration, remote mode, the
toolchain model and the CLI reference. Design and measurements: [`docs/design.md`](docs/design.md).

## License

Apache-2.0.
