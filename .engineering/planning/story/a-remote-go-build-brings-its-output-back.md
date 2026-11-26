---
format: aep.planning-md/3
id: story:a-remote-go-build-brings-its-output-back
kind: story
status: draft
title: A remote go build brings its output back
revision: 1
---
## Outcome

The file a remote `go build -o <path>` or `go test -c -o <path>` wrote is on the workstation
after the job, at the same path, also when the path lies under `target/`.

## Why

Products of a remote job come back two ways. The client brings back the products of a cargo
job (`crates/slot/src/artifacts.rs`); it knows no go command. A wrapper around the client may
bring back what the job wrote in the tree, and leaves out `target/`, `node_modules/` and
`.git/`, because a cargo target directory must not travel. A task that builds a go binary into
`target/` on a node and runs it on the workstation in its next step then finds no file.
Probe: a job that wrote `target/<dir>/out.txt` and `<other dir>/out.txt` on the node; after
it the second file was on the workstation, the first was not.

`slot --fetch <path> <command>` brings such a path back today; a `go build` that reaches slot
through the PATH shim carries no `--fetch`.

## Acceptance

- Red first: a remote `go build -o target/x/tool ./cmd/tool` that exits 0 leaves no
  `target/x/tool` on the client's side.
- After the change the file is there with its mode and mtime; a relative `-o` is read against
  the job's working directory; `-o` naming a directory brings back the files the build wrote
  into it and no other file of that directory.
- An `-o` outside the tree, and a job that did not exit 0, bring nothing back and print no
  error.
- `SLOT_NO_PRODUCTS=1` turns it off, as for cargo.
- The time of a remote `go build` without `-o` is unchanged: measured before and after.
