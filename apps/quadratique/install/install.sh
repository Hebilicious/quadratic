#!/bin/sh
set -eu

binary=${1:?usage: install.sh <quadratique binary>}
: "${HOME:?HOME is not set}"
bin_home=${XDG_BIN_HOME:-$HOME/.local/bin}
units=$HOME/.config/systemd/user
systemctl=${QUADRATIQUE_SYSTEMCTL:-systemctl}
installed=$bin_home/quadratique
unit=$units/quadratique.service

if [ -n "${XDG_CONFIG_HOME:-}" ] && [ "$XDG_CONFIG_HOME" != "$HOME/.config" ]; then
  echo "install.sh: XDG_CONFIG_HOME is $XDG_CONFIG_HOME, not ~/.config; the systemd user manager searches it only when its own environment sets it. Run the install with XDG_CONFIG_HOME unset." >&2
  exit 1
fi

refuse_in_unit() {
  case $1 in
    *[\"\\%\$]* | *[[:cntrl:]]*)
      echo "install.sh: $1 has a character that a systemd unit would read differently (\" \\ % \$ or a control character)" >&2
      exit 1
      ;;
  esac
}
refuse_in_unit "$installed"
refuse_in_unit "${XDG_DATA_HOME:-}"

for destination in "$installed" "$unit"; do
  if [ -d "$destination" ]; then
    echo "install.sh: $destination is a folder; move it away first." >&2
    exit 1
  fi
done

mkdir -p "$bin_home" "$units"
staged_binary=$(mktemp "$bin_home/.quadratique.XXXXXX")
staged_unit=$(mktemp "$units/.quadratique.service.XXXXXX")
trap 'rm -f "$staged_binary" "$staged_unit"' EXIT
cat "$binary" > "$staged_binary"
chmod 755 "$staged_binary"
mv -f -T -- "$staged_binary" "$installed"

{
  echo "[Unit]"
  echo "Description=Quadratique server"
  echo "StartLimitIntervalSec=0"
  echo
  echo "[Service]"
  if [ -n "${XDG_DATA_HOME:-}" ]; then
    echo "Environment=\"XDG_DATA_HOME=$XDG_DATA_HOME\""
  fi
  echo "ExecStart=\"$installed\" serve --take-over"
  echo "Restart=on-failure"
  echo "RestartSec=2"
  echo "RestartPreventExitStatus=5"
  echo "TimeoutStopSec=70"
  echo
  echo "[Install]"
  echo "WantedBy=default.target"
} > "$staged_unit"
chmod 644 "$staged_unit"
mv -f -T -- "$staged_unit" "$unit"

"$systemctl" --user daemon-reload
"$systemctl" --user enable quadratique.service
not_ready() {
  echo "install.sh: $1" >&2
  "$systemctl" --user status quadratique.service >&2 || true
  exit 1
}

"$systemctl" --user restart quadratique.service || not_ready "systemctl --user restart quadratique.service failed:"

as_service() {
  if [ -n "${XDG_DATA_HOME:-}" ]; then
    env -u QUADRATIQUE_HOME XDG_DATA_HOME="$XDG_DATA_HOME" "$@"
  else
    env -u QUADRATIQUE_HOME -u XDG_DATA_HOME "$@"
  fi
}

installed_file=$(readlink -f -- "$installed")

ready() {
  main=$("$systemctl" --user show -p MainPID --value quadratique.service 2> /dev/null) || return 1
  case $main in '' | 0 | *[!0-9]*) return 1 ;; esac
  [ "$(readlink -- "/proc/$main/exe" 2> /dev/null)" = "$installed_file" ] || return 1
  status=$(as_service "$installed" server status 2> /dev/null) || return 1
  case $status in *'"running":true'*) ;; *) return 1 ;; esac
  case $status in *'"sameInterface":true'*) ;; *) return 1 ;; esac
  case $status in *'"pid":'"$main"[,}]*) ;; *) return 1 ;; esac
}

deadline=$(($(date +%s) + 60))
until ready; do
  if [ "$("$systemctl" --user is-active quadratique.service || true)" = failed ] || [ "$(date +%s)" -ge "$deadline" ]; then
    not_ready "the unit's main process is not a server of $installed that answers for its home:"
  fi
  sleep 0.1
done

echo "Installed $installed and $unit."
case ":${PATH:-}:" in
  *":$bin_home:"*) ;;
  *) echo "$bin_home is not on PATH: add it to run quadratique by name." >&2 ;;
esac
