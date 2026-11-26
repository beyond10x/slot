#!/bin/sh
# shell-tests.sh <test script>...: runs each shell test once and stops at the first that fails.
# The forge's `sh` is dash, a workstation's or a node's may be bash, and a test can pass under
# one and fail under the other. Where dash is on PATH the tests run under dash: a directory whose
# one entry is `sh`, a link to dash, goes first on PATH, so the `sh` that a test starts by name is
# dash too. Without dash they run under the machine's sh. The first line says which shell it is.
# What still runs the machine's own sh: a script started through its `#!/bin/sh` line, and a
# command whose PATH a test sets itself.
set -u
[ $# -gt 0 ] || {
  echo "usage: shell-tests.sh <test script>..." >&2
  exit 2
}
d=$(command -v dash 2>/dev/null) || d=
case $d in
  /*)
    b=$(mktemp -d) || exit 1
    trap 'rm -rf "$b"' EXIT
    trap 'exit 1' HUP INT TERM
    ln -s "$d" "$b/sh" || exit 1
    PATH=$b:$PATH
    export PATH
    echo "shell tests: under dash ($d), which is sh for the tests"
    ;;
  *) echo "shell tests: under sh ($(readlink -f "$(command -v sh)")): no dash on PATH" ;;
esac
for t in "$@"; do
  sh "$t" || exit 1
done
