#!/bin/sh
# Installs a release archive as a person does, in an empty home, and uses the installed binary: it checks the archive
# against its SHA-256 file, unpacks it, runs its install.sh, and then the binary gives its version, serves the web app
# and answers commands. On macOS the archive is first marked as a browser marks a download (com.apple.quarantine), and
# the installed binary must not carry that mark. It writes nothing outside its temporary folders.
# Usage: try-archive.sh <archive.tar.gz> <version>
set -eu
usage="usage: try-archive.sh <archive.tar.gz> <version>"
archive=${1:?$usage}
version=${2:?$usage}
name=$(basename "$archive" .tar.gz)

scratch=$(mktemp -d)
# The server's socket is in the data folder, and its path may be at most 103 bytes on macOS: the data folder is short.
data_parent=$(mktemp -d /tmp/qtry.XXXXXX)
home=$scratch/home
data=$data_parent/home
installed=$home/.local/bin/quadratique

on_machine() {
  env -i PATH="/usr/bin:/bin:/usr/sbin:/sbin" HOME="$home" USER=tester QUADRATIQUE_HOME="$data" "$@"
}

cleanup() {
  [ -x "$installed" ] && on_machine "$installed" server stop > /dev/null 2>&1 || true
  rm -rf "$scratch" "$data_parent"
}
trap cleanup EXIT INT TERM

fail() {
  echo "FAIL: $*" >&2
  [ -f "$data/server.log" ] && cat "$data/server.log" >&2
  exit 1
}

pass() {
  echo "ok: $*"
}

mkdir -p "$home" "$scratch/download"
cp "$archive" "$archive.sha256" "$scratch/download/"
cd "$scratch/download"
if command -v sha256sum > /dev/null; then
  sha256sum -c "$name.tar.gz.sha256" || fail "the archive does not match its SHA-256 file"
else
  shasum -a 256 -c "$name.tar.gz.sha256" || fail "the archive does not match its SHA-256 file"
fi
pass "the archive matches its SHA-256 file"

quarantined=no
if [ "$(uname -s)" = Darwin ]; then
  xattr -w com.apple.quarantine "0081;$(printf '%x' "$(date +%s)");Safari;" "$name.tar.gz"
  quarantined=yes
fi

tar -xzf "$name.tar.gz"
if [ "$quarantined" = yes ]; then
  if xattr -p com.apple.quarantine "$name/bin/quadratique" > /dev/null 2>&1; then
    echo "note: tar carried the download's quarantine mark to the unpacked $name/bin/quadratique"
  else
    echo "note: tar did not carry the download's quarantine mark to the unpacked $name/bin/quadratique"
  fi
fi

on_machine sh "$name/install.sh" "$name/bin/quadratique" || fail "install.sh fails"
[ -x "$installed" ] || fail "install.sh did not install $installed"
pass "install.sh installs the archive's binary in an empty home, at $installed"

if [ "$quarantined" = yes ]; then
  if xattr -p com.apple.quarantine "$installed" > /dev/null 2>&1; then
    fail "the installed binary carries the quarantine mark of the download"
  fi
  pass "the installed binary carries no quarantine mark, though the archive was downloaded as a browser does"
fi

said=$(on_machine "$installed" --version) || fail "the installed binary does not run"
[ "$said" = "$version" ] || fail "the installed binary says it is $said, not $version"
pass "the installed binary says it is $version"

url=$(on_machine "$installed" ui | sed -n 's/.*"url": *"\([^"]*\)".*/\1/p')
[ -n "$url" ] || fail "quadratique ui gave no address"
curl -fsS "$url" > "$scratch/page.html" || fail "the installed binary does not serve $url"
grep -qi '<!doctype html>' "$scratch/page.html" || fail "$url is not the web app's page"
pass "the installed binary serves the web app's page at $url"

on_machine "$installed" project create --key TRY --name Tried > /dev/null || fail "project create fails"
on_machine "$installed" create --project TRY --title "From the archive" --acceptance "It is shown." > /dev/null \
  || fail "create fails"
on_machine "$installed" show --issue TRY-1 --json | grep -q '"title": *"From the archive"' \
  || fail "show does not give the issue that create made"
on_machine "$installed" server stop > /dev/null || fail "server stop fails"
pass "the installed binary creates a project and an issue, shows it, and stops its server"
