#!/bin/sh
set -eu
usage="usage: package.sh <quadratique binary> <version> <target triple> <output folder>"
binary=${1:?$usage}
version=${2:?$usage}
target=${3:?$usage}
output=${4:?$usage}
here=$(cd "$(dirname "$0")" && pwd)
repository=$(cd "$here/../../.." && pwd)
name=quadratique-$version-$target

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/$name/bin"
cp "$binary" "$stage/$name/bin/quadratique"
cp "$here/install.sh" "$stage/$name/"
cp "$here/../README.md" "$stage/$name/README.md"
cp "$repository/LICENSE" "$stage/$name/LICENSE"
chmod 755 "$stage/$name/bin/quadratique" "$stage/$name/install.sh"
chmod 644 "$stage/$name/README.md" "$stage/$name/LICENSE"

mkdir -p "$output"
tar -C "$stage" --sort=name --owner=0 --group=0 --numeric-owner -czf "$output/$name.tar.gz" "$name"
(cd "$output" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
echo "$output/$name.tar.gz"
