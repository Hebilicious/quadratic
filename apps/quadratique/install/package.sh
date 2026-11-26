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

# Both tars get the same entries in the same order: the sorted list, without recursion. Only the flags that make
# root the owner differ; macOS's bsdtar also leaves out the file attributes and AppleDouble (._) files of macOS.
(cd "$stage" && find "$name" | LC_ALL=C sort) > "$stage/entries"
if tar --version 2> /dev/null | grep -q 'GNU tar'; then
  owner="--owner=0 --group=0 --numeric-owner"
else
  owner="--uid 0 --gid 0 --numeric-owner --no-xattrs --no-acls --no-mac-metadata"
fi
mkdir -p "$output"
# shellcheck disable=SC2086 # $owner is a list of flags.
COPYFILE_DISABLE=1 tar -C "$stage" $owner --no-recursion -T "$stage/entries" -czf "$output/$name.tar.gz"
# sha256sum and macOS's shasum -a 256 print the same line: the hash, two spaces and the name.
if command -v sha256sum > /dev/null; then sha256="sha256sum"; else sha256="shasum -a 256"; fi
(cd "$output" && $sha256 "$name.tar.gz" > "$name.tar.gz.sha256")
echo "$output/$name.tar.gz"
