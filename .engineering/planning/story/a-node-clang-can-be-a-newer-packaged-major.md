---
format: aep.planning-md/3
id: story:a-node-clang-can-be-a-newer-packaged-major
kind: story
status: draft
title: A node's clang on PATH can be a newer packaged major
relations:
- serves: vision:O2
revision: 1
---
## Outcome

A build node's `clang` on a job's PATH can be a newer packaged major than the distribution's
default: a node config with `CLANG_VERSION=<major>` gets the packages `clang<major>` and
`clang<major>-libs`, and `clang`, `clang++` and `clang-cpp` in `/usr/local/bin` link to
`/usr/bin/clang-<major>`, `clang++-<major>` and `clang-cpp-<major>`.

## Why

Amazon Linux 2023's default `clang` is 15.0.7; the workstation's is a current major. A consumer
that runs `clang -Xclang -ast-dump=json -fsyntax-only -std=c11` from PATH fails on a node with
clang 15's JSON syntax tree. The distribution packages newer majors (`clang20` to `clang22`)
beside the default without putting them on PATH as `clang`. `/usr/local/bin` is first on a
job's PATH, so links there choose the major without replacing the distribution's `/usr/bin/clang`.

## Acceptance

1. `conf.sh` refuses a `CLANG_VERSION` that is not digits (or has a leading zero), also for a
   bundle; an empty or absent key installs no clang package, sets no link and checks nothing.
2. With `CLANG_VERSION=22`, `provision.sh check` names the missing packages `clang22
   clang22-libs`, each missing or wrong link in `/usr/local/bin`, a real file at a link's name,
   and a `clang --version`, run as the build user from `/usr/local/bin`, that does not name
   `clang version 22.`.
3. `provision.sh apply` installs the packages and sets the links, only to a binary that is
   there; it leaves a real file at a link's name and reports it `failed` (exit 1), and reports a
   clang of another major after the links as `failed`. A second apply changes nothing; another
   major moves the links.
4. Taking the key out of the config leaves the links as they are; the README says so.
5. The tests are a section of `contrib/node/test.sh`; `node.conf.example` carries
   `CLANG_VERSION=22`, the README has its row and section, and `CHANGELOG.md` an entry.
