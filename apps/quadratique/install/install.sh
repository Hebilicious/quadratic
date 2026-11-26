#!/bin/sh
# Puts the quadratique binary on the PATH: copies it to $XDG_BIN_HOME/quadratique, or ~/.local/bin/quadratique. It
# starts nothing: the first command starts the server. The README says how to keep the server up after a reboot.
set -eu

fail() {
  echo "install.sh: $*" >&2
  exit 1
}

binary=${1:-}
[ -n "$binary" ] || fail "give the quadratique binary to install: sh install.sh <binary>"
[ -f "$binary" ] && [ -r "$binary" ] || fail "$binary is not a file this account can read: give the path of the quadratique binary"
[ -n "${HOME:-}" ] || fail "HOME is not set: set it to your home folder"
# The XDG Base Directory specification: a path that is not absolute is invalid, and is ignored.
case ${XDG_BIN_HOME:-} in
  /*) bin_home=$XDG_BIN_HOME ;;
  *) bin_home=$HOME/.local/bin ;;
esac
installed=$bin_home/quadratique

[ ! -d "$installed" ] || fail "$installed is a folder; move it away first"
mkdir -p "$bin_home" 2> /dev/null || fail "cannot make the folder $bin_home: check the permissions of the folders above it, or set XDG_BIN_HOME to a folder of yours"
staged=$(mktemp "$bin_home/.quadratique.XXXXXX" 2> /dev/null) \
  || fail "cannot write in $bin_home: check its permissions, or set XDG_BIN_HOME to a folder of yours"
trap 'rm -f "$staged"' EXIT
cat "$binary" > "$staged" 2> /dev/null || fail "cannot copy $binary into $bin_home: check that the disk has room"
chmod 755 "$staged"
replaced=no
[ -e "$installed" ] && replaced=yes
# A rename replaces the file whole: a server that runs the earlier file keeps it until it stops. GNU mv's -T refuses
# to move the file into a folder made at that name after the check above; macOS's mv has no -T, so there a folder made
# in between is found after the move, and the moved file is taken back out of it.
if mv --version > /dev/null 2>&1; then
  mv -f -T -- "$staged" "$installed" 2> /dev/null || fail "cannot replace $installed: check its permissions"
else
  mv -f -- "$staged" "$installed" 2> /dev/null || fail "cannot replace $installed: check its permissions"
  if [ -d "$installed" ]; then
    rm -f "$installed/$(basename "$staged")"
    fail "$installed became a folder during the install; move it away and run install.sh again"
  fi
fi
trap - EXIT

echo "Installed $installed."
# Only after a replacement can a server run the earlier file. `server status` starts no server.
if [ "$replaced" = yes ]; then
  case $("$installed" server status 2> /dev/null || true) in
    *'"running":true'*)
      echo "A Quadratique server runs for this data folder: it keeps running the earlier build until \`quadratique server stop\`, and the next command then starts this one."
      ;;
  esac
fi
case ":${PATH:-}:" in
  *":$bin_home:"*) ;;
  *) echo "$bin_home is not on PATH: add it to run quadratique by name." >&2 ;;
esac
