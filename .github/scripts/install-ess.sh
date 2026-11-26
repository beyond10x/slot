#!/bin/sh
# Installs the `ess` release spec/ess-inputs.yaml requires (`requires: ess X.Y.Z`) for the gate's
# UI-spec check: the x86_64 Linux build from the release's assets, checked against its SHA256SUMS.
#
#   install-ess.sh DIR   unpacks `ess` into DIR and prints DIR (for $GITHUB_PATH)
set -eu

dir=${1:?usage: install-ess.sh DIR}
version=$(sed -n 's/^requires: ess \([0-9][0-9.]*\)$/\1/p' spec/ess-inputs.yaml)
[ -n "$version" ] || { echo "install-ess: spec/ess-inputs.yaml names no \`requires: ess X.Y.Z\`" >&2; exit 1; }
asset="ess-$version-x86_64-unknown-linux-gnu.tar.gz"
base="https://github.com/beyond10x/ess/releases/download/$version"

mkdir -p "$dir"
cd "$dir"
curl -fsSL --retry 3 -o "$asset" "$base/$asset"
curl -fsSL --retry 3 -o SHA256SUMS "$base/SHA256SUMS"
grep " $asset\$" SHA256SUMS | sha256sum -c - >&2
tar -xzf "$asset" --strip-components=1 "ess-$version-x86_64-unknown-linux-gnu/ess"
./ess --version >&2
pwd
