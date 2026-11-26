#!/bin/sh
# Puts the quadratique binary on the PATH: copies it to $XDG_BIN_HOME/quadratique, or ~/.local/bin/quadratique. It
# starts nothing: the first command starts the server. The README says how to keep the server up after a reboot.
set -eu

binary=${1:?usage: install.sh <quadratique binary>}
: "${HOME:?HOME is not set}"
bin_home=${XDG_BIN_HOME:-$HOME/.local/bin}
installed=$bin_home/quadratique

if [ -d "$installed" ]; then
  echo "install.sh: $installed is a folder; move it away first." >&2
  exit 1
fi

mkdir -p "$bin_home"
staged=$(mktemp "$bin_home/.quadratique.XXXXXX")
trap 'rm -f "$staged"' EXIT
cat "$binary" > "$staged"
chmod 755 "$staged"
# A rename replaces the file whole: a server that runs the old file keeps it until it stops.
mv -f -- "$staged" "$installed"

echo "Installed $installed."
case ":${PATH:-}:" in
  *":$bin_home:"*) ;;
  *) echo "$bin_home is not on PATH: add it to run quadratique by name." >&2 ;;
esac
