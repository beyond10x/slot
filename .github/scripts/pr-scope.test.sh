#!/bin/sh
# Test of pr-scope.sh: path lists from each side of the planning-or-prose list,
# and the git route a pull request takes in CI.
#
# Run from anywhere: sh .github/scripts/pr-scope.test.sh
# Scratch files go under target/ of the repository, or under $PR_SCOPE_TEST_DIR.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
script=$here/pr-scope.sh
failures=0
cases=0

# expect <want> <description> <path>...: feed the paths, one per line, to --classify.
expect() {
  want=$1
  what=$2
  shift 2
  cases=$((cases + 1))
  got=$(printf '%s\n' "$@" | sh "$script" --classify 2>/dev/null) || got="exit $?"
  if [ "$got" = "scope=$want" ]; then
    echo "ok   $what: $got"
  else
    echo "FAIL $what: want scope=$want, got $got"
    failures=$((failures + 1))
  fi
}

expect none 'only planning files' \
  .engineering/planning/story/a.md .engineering/planning/wave/w1.md
expect none 'planning files and prose outside website/ and ui/' \
  .engineering/planning/story/a.md AGENTS.md CHANGELOG.md docs/design.md plugin/skills/init/SKILL.md
expect none 'a planning file that is not markdown' \
  .engineering/planning/index.json
expect code 'planning files plus one source file' \
  .engineering/planning/story/a.md AGENTS.md crates/slot/src/lib.rs
expect code 'planning files plus the toolchain file' \
  .engineering/planning/story/a.md rust-toolchain.toml
expect code 'a workflow' \
  .github/workflows/gate.yml
expect code 'a UX finding under ui/, read by the UI tests' \
  ui/ux/findings/UXF-001.md
expect code 'a path that only starts like the planning directory' \
  .engineering-notes/a.rs
expect docs 'only website/' \
  website/docs/how-it-works.md website/src/css/custom.css
expect code 'the getting-started page, read by cargo test' \
  website/docs/getting-started.md
expect docs 'website/ and planning files' \
  website/docs/toolchains.md .engineering/planning/story/a.md README.md
expect code 'website/ plus one source file' \
  website/docs/toolchains.md crates/slot-cli/src/main.rs
expect code 'no paths at all' \
  ''
expect code 'one source file listed before a website/ file' \
  crates/slot-cli/src/main.rs website/docs/toolchains.md

# Events other than a pull request always build.
cases=$((cases + 1))
got=$(sh "$script" push 2>/dev/null) || got="exit $?"
if [ "$got" = scope=code ]; then
  echo "ok   a push builds: $got"
else
  echo "FAIL a push builds: want scope=code, got $got"
  failures=$((failures + 1))
fi

# The git route: in CI a pull request is checked out as a merge commit whose first
# parent is the base branch; the change is the merge against that parent.
base=${PR_SCOPE_TEST_DIR:-$root/target}
mkdir -p "$base"
work=$(mktemp -d "$base/pr-scope.XXXXXX")
trap 'rm -rf "$work"' EXIT INT TERM

g() {
  git -C "$repo" -c core.hooksPath=/dev/null -c commit.gpgsign=false \
    -c user.name=test -c user.email=test@example.invalid "$@"
}

# pr <name> <commands for the pull request branch>: build a repository whose main
# moved on after the branch was cut, merge the branch, and print the script's answer.
pr() {
  repo=$work/$1
  mkdir -p "$repo"
  g init -q -b main
  mkdir -p "$repo/src"
  echo a > "$repo/src/lib.rs"
  echo a > "$repo/README.md"
  g add -A
  g commit -q -m base
  g checkout -q -b pr
  (cd "$repo" && eval "$2")
  g add -A
  g commit -q -m change
  g checkout -q main
  echo b > "$repo/other.rs"
  g add -A
  g commit -q -m 'main moved on'
  g merge -q --no-ff -m merge pr
  (cd "$repo" && sh "$script" pull_request 2>/dev/null) || echo "exit $?"
}

expect_pr() {
  want=$1
  what=$2
  cases=$((cases + 1))
  got=$(pr "case$cases" "$3" 2>&1) || got="setup failed: $got"
  if [ "$got" = "scope=$want" ]; then
    echo "ok   pull request, $what: $got"
  else
    echo "FAIL pull request, $what: want scope=$want, got $got"
    failures=$((failures + 1))
  fi
}

expect_pr none 'planning files only, base moved on with source' \
  'mkdir -p .engineering && echo x > .engineering/a.md && echo b > README.md'
expect_pr code 'planning files plus one source file' \
  'mkdir -p .engineering && echo x > .engineering/a.md && echo b > src/lib.rs'
expect_pr code 'a source file renamed into the planning directory' \
  'mkdir -p .engineering && git mv src/lib.rs .engineering/lib.md'

echo "$cases cases, $failures failed"
[ "$failures" -eq 0 ]
