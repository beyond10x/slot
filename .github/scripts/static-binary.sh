#!/bin/sh
# Proves that a binary is statically linked: its ELF headers name no shared library (no
# NEEDED entry in a dynamic section) and no program interpreter. A static-pie binary has a
# dynamic section and neither of the two; it passes.
#
#   static-binary.sh FILE   prints `static: FILE` and exits 0, or says on standard error why
#                           FILE is not static and exits 1
set -eu

bin=${1:?usage: static-binary.sh FILE}
LC_ALL=C
export LC_ALL

refuse() {
  echo "static-binary: $bin is not static: $1" >&2
  exit 1
}

segments=$(readelf -lW "$bin" 2>/dev/null) || refuse "readelf reads no ELF program headers in it"
dynamic=$(readelf -dW "$bin" 2>/dev/null) || refuse "readelf reads no ELF file in it"
case $segments in
  *"Program Headers:"*) ;;
  *) refuse "it has no program headers" ;;
esac
needed=$(printf '%s\n' "$dynamic" | grep -F '(NEEDED)' || true)
[ -z "$needed" ] || refuse "it names shared libraries:
$needed"
interp=$(printf '%s\n' "$segments" | grep -E '^ +INTERP |program interpreter' || true)
[ -z "$interp" ] || refuse "it names a program interpreter:
$interp"
echo "static: $bin"
