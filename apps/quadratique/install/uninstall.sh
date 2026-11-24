#!/bin/sh
set -eu

: "${HOME:?HOME is not set}"
systemctl=${QUADRATIQUE_SYSTEMCTL:-systemctl}
unit=$HOME/.config/systemd/user/quadratique.service

if [ ! -e "$unit" ]; then
  echo "uninstall.sh: there is no $unit, so Quadratique is not installed for this user." >&2
  exit 0
fi

installed=$(sed -n 's/^ExecStart="\([^"]*\)" serve --take-over$/\1/p' "$unit")
data_home=$(sed -n 's/^Environment="XDG_DATA_HOME=\([^"]*\)"$/\1/p' "$unit")
if [ -z "$installed" ]; then
  echo "uninstall.sh: $unit has no ExecStart line that install.sh writes; remove it by hand." >&2
  exit 1
fi

as_service() {
  if [ -n "$data_home" ]; then
    env -u QUADRATIQUE_HOME XDG_DATA_HOME="$data_home" "$@"
  else
    env -u QUADRATIQUE_HOME -u XDG_DATA_HOME "$@"
  fi
}

home_directory=${data_home:-$HOME/.local/share}/quadratique
if [ -d "$home_directory" ]; then
  exec 8>> "$home_directory/start.lock"
  if ! flock -w 60 8; then
    echo "uninstall.sh: a Quadratique server is starting for $home_directory; nothing was removed. Try again." >&2
    exit 1
  fi
  "$systemctl" --user stop quadratique.service
  if [ -x "$installed" ]; then
    if ! as_service "$installed" server stop > /dev/null; then
      echo "uninstall.sh: the Quadratique server did not stop; nothing was removed. See \`$installed server status\`." >&2
      exit 1
    fi
  else
    echo "uninstall.sh: $installed is missing, so it could not stop a server; checking that none holds $home_directory." >&2
  fi
  exec 9>> "$home_directory/server.lock"
  if ! flock -w 10 9; then
    echo "uninstall.sh: a Quadratique server still holds $home_directory; nothing was removed." >&2
    exit 1
  fi
fi
"$systemctl" --user disable quadratique.service
rm -f "$unit"
"$systemctl" --user daemon-reload
rm -f "$installed"

echo "Removed $installed and $unit. The store is untouched."
