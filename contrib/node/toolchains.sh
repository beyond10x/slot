#!/bin/sh
# toolchains.sh <usr-prefix> <rustup-home>: one line per toolchain a build job can resolve to.
#   rust <name> <rustc commit hash> <digest of the installed targets and components>
#   go <digest of the go binary> / node <digest of the node binary>
# Run on the workstation with prefix "" and on the node with the prefix of its copy of /usr; the
# workstation holds a node job while a line its tree uses differs between the two.
export LC_ALL=C
P=$1
R=${2:-$HOME/.rustup}
for d in "$R"/toolchains/*; do
  [ -x "$d/bin/rustc" ] || continue
  n=$(basename "$d")
  h=$("$d/bin/rustc" -vV 2>/dev/null | sed -n 's/^commit-hash: //p')
  c=$(ls "$d/lib/rustlib" 2>/dev/null | grep -v '^multirust\|^rust-installer-version$' | sort | md5sum | cut -c1-12)
  echo "rust $n ${h:-none} $c"
done
for b in /usr/lib/go/bin/go /usr/bin/node; do
  [ -f "$P$b" ] || continue
  echo "$(basename "$b") $(sha256sum "$P$b" | cut -c1-16)"
done
# C side: the compilers and cross compilers a job resolves through /usr/bin, and the compile cache
# server that runs them (the process listening on 4226). Links resolve inside the prefix.
for n in cc gcc clang sccache x86_64-w64-mingw32-gcc musl-gcc; do
  r=$P/usr/bin/$n
  i=0
  while [ -L "$r" ] && [ $i -lt 10 ]; do
    l=$(readlink "$r")
    case $l in /*) r=$P$l ;; *) r=$(dirname "$r")/$l ;; esac
    i=$((i + 1))
  done
  if [ -f "$r" ]; then echo "c $n $(sha256sum "$r" | cut -c1-16)"; else echo "c $n missing"; fi
done
if [ -d "$P/usr/x86_64-w64-mingw32" ]; then
  echo "c mingw-sysroot $(cd "$P/usr/x86_64-w64-mingw32" && find . -type f | sort | md5sum | cut -c1-12)"
else
  echo "c mingw-sysroot missing"
fi
# The whole /usr a job sees: path, type, size, mode and mtime of every entry (entries the build user cannot
# read are left out: lib/rkhunter, var and 25 root-only files such as factory/etc/shadow).
u=$(cd "$P/usr" && find . \( -path ./lib/rkhunter -o -path ./var -o \( -type f ! -readable \) -o \( -type d ! -readable \) \) -prune -o \( -type f -printf '%p f %s %m %T@\n' \) -o \( ! -type f -printf '%p %y %m %T@\n' \) 2>/dev/null |
  sed 's/\.[0-9]*$//' | sort | md5sum | cut -c1-12)
echo "c usr $u"
# The /etc files node jobs read from this machine (bound read-only copies): path and content.
e=$(cd "$P/etc" && find -H fonts localtime locale.conf ssl ca-certificates nsswitch.conf ld.so.conf ld.so.conf.d \
  mime.types protocols services os-release -path ssl/private -prune -o \( -type l -printf "%l  %p\\n" \) -o -type f -readable -exec md5sum {} + 2>/dev/null |
  sort -k2 | md5sum | cut -c1-12)
echo "c etc $e"
s=$(ss -ltnpH 'sport = :4226' 2>/dev/null | sed -n 's/.*pid=\([0-9]*\).*/\1/p' | head -1)
if [ -n "$s" ] && [ -r "/proc/$s/exe" ]; then echo "c sccache-server $(sha256sum "/proc/$s/exe" | cut -c1-16)"; else echo "c sccache-server none"; fi
