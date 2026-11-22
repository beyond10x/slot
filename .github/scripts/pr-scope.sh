#!/bin/sh
# Which checks a change needs. Prints one line for $GITHUB_OUTPUT, `scope=<scope>`:
#
#   none  only planning or prose files: everything under .engineering/, and *.md
#         outside website/ and ui/ (ui/ux/findings/*.md are read by the UI tests)
#   docs  those and files under website/: the docs checks, not the code's build and tests
#   code  anything else, or no paths at all: every check
#
# This case block is the one path list of the repository.
#
#   pr-scope.sh EVENT       in CI: for a pull_request, the paths of the merge commit
#                           against its first parent (the base); any other event: code
#   pr-scope.sh --classify  the paths on stdin, one per line
#
# Every path and the answer go to stderr, so the job's log names what it saw.
set -eu

kind() {
  case $1 in
    .engineering/*) echo none ;;
    website/*) echo docs ;;
    ui/*) echo code ;;
    *.md) echo none ;;
    *) echo code ;;
  esac
}

classify() {
  scope=none
  seen=0
  while IFS= read -r path || [ -n "$path" ]; do
    [ -n "$path" ] || continue
    seen=$((seen + 1))
    k=$(kind "$path")
    echo "changed ($k): $path" >&2
    case $k in
      code) scope=code ;;
      docs) [ "$scope" = code ] || scope=docs ;;
    esac
  done
  [ "$seen" -gt 0 ] || scope=code
  echo "scope: $scope, from $seen changed paths" >&2
  echo "scope=$scope"
}

case ${1:-} in
  --classify)
    classify
    ;;
  pull_request)
    # --no-renames: a file moved into .engineering/ also counts where it came from.
    changed=$(git diff --no-renames --name-only HEAD^1 HEAD)
    printf '%s\n' "$changed" | classify
    ;;
  '')
    echo "usage: pr-scope.sh EVENT | --classify" >&2
    exit 2
    ;;
  *)
    echo "event $1: not a pull request, every check runs" >&2
    echo "scope=code"
    ;;
esac
