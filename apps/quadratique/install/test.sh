#!/bin/sh
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
scratch=$(mktemp -d)
case $scratch in *[[:space:]]*) echo "the temporary folder $scratch has a space" >&2; exit 1 ;; esac
home=$scratch/home

processes_of_the_home() {
  for process in /proc/[0-9]*; do
    if { tr '\0' '\n' < "$process/environ"; } 2> /dev/null | grep -qx "HOME=$home"; then
      echo "${process#/proc/}"
    fi
  done
}

cleanup() {
  for pid in $(processes_of_the_home); do kill -KILL "$pid" 2> /dev/null || true; done
  rm -rf "$scratch"
}
trap cleanup EXIT INT TERM

show_file() {
  echo "--- $1" >&2
  if [ -f "$1" ]; then cat "$1" >&2; else echo "(none)" >&2; fi
}

fail() {
  echo "FAIL: $*" >&2
  show_file "$systemd_state/service.log"
  show_file "$systemd_state/calls"
  show_file "$data_home/quadratique/server.log"
  show_file "$data_home/quadratique/server.json"
  show_file "$scratch/install.log"
  exit 1
}

pass() {
  echo "ok: $*"
}

bin_home="$home/my bin"
quadratique=$bin_home/quadratique
unit=$home/.config/systemd/user/quadratique.service
data_home="$home/my data"
systemd_state=$scratch/systemd
mkdir -p "$home" "$systemd_state"

cat > "$scratch/systemctl" <<'FAKE'
#!/bin/sh
set -eu
[ "$1" = --user ] || { echo "fake systemctl: only --user" >&2; exit 2; }
shift
state=$FAKE_SYSTEMD
unit=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/quadratique.service
echo "$*" >> "$state/calls"

started_at() {
  process_stat=$(cat "/proc/$1/stat" 2> /dev/null) || return 1
  set -- ${process_stat##*) }
  [ "$1" != Z ] || return 1
  eval "echo \${20}"
}

running() {
  [ -f "$state/pid" ] && [ -s "$state/started" ] \
    && [ "$(started_at "$(cat "$state/pid")")" = "$(cat "$state/started")" ]
}

stop() {
  if running; then
    pid=$(cat "$state/pid")
    kill -TERM "$pid" 2> /dev/null || true
    deadline=$(($(date +%s) + 120))
    while running; do
      [ "$(date +%s)" -lt "$deadline" ] || { echo "fake systemctl: $pid did not stop within 120 s" >&2; exit 1; }
      sleep 0.05
    done
  fi
  rm -f "$state/pid" "$state/started"
}

start() {
  [ -f "$state/broken" ] && return
  assignment=$(sed -n 's/^Environment="\{0,1\}\([^"]*\)"\{0,1\}$/\1/p' "$unit")
  command=$(sed -n 's/^ExecStart=//p' "$unit")
  if [ -n "$assignment" ]; then
    env "$assignment" sh -c "exec $command" < /dev/null >> "$state/service.log" 2>&1 &
  else
    sh -c "exec $command" < /dev/null >> "$state/service.log" 2>&1 &
  fi
  echo $! > "$state/pid"
  started_at $! > "$state/started" || true
}

case $1 in
  daemon-reload) ;;
  enable) touch "$state/enabled" ;;
  restart)
    [ -f "$state/restart-fails" ] && exit 1
    [ -f "$state/restart-does-nothing" ] && exit 0
    stop
    start
    ;;
  show) [ "${2:-}" = -p ] && [ "${3:-}" = MainPID ] && { if running; then cat "$state/pid"; else echo 0; fi; } ;;
  stop) stop ;;
  disable)
    rm -f "$state/enabled"
    if [ "${2:-}" = --now ]; then stop; fi
    ;;
  is-active) if running; then echo active; else echo failed; exit 3; fi ;;
  status) if running; then echo "fake: running, pid $(cat "$state/pid")"; else echo "fake: failed"; fi; tail -n 5 "$state/service.log" 2> /dev/null || true ;;
  *) echo "fake systemctl: no verb $1" >&2; exit 2 ;;
esac
FAKE
chmod +x "$scratch/systemctl"

exec_on_machine() {
  exec env -i PATH="$PATH" HOME="$home" USER=tester XDG_BIN_HOME="$bin_home" XDG_DATA_HOME="$data_home" \
    XDG_RUNTIME_DIR="$home/run" QUADRATIQUE_SYSTEMCTL="$scratch/systemctl" FAKE_SYSTEMD="$systemd_state" "$@"
}

on_machine() {
  (exec_on_machine "$@")
}

owner() {
  on_machine "$quadratique" server status | jq -r 'select(.running and .sameInterface) | .pid'
}

service() {
  cat "$systemd_state/pid"
}

servers() {
  count=0
  for pid in $(processes_of_the_home); do
    if { tr '\0' ' ' < "/proc/$pid/cmdline"; } 2> /dev/null | grep -q "quadratique serve"; then
      count=$((count + 1))
    fi
  done
  echo "$count"
}

await_servers() {
  deadline=$(($(date +%s) + 120))
  until [ "$(servers)" = "$1" ]; do
    [ "$(date +%s)" -lt "$deadline" ] || fail "$(servers) servers run for the home $2, not $1"
    sleep 0.05
  done
}

alive() {
  process_stat=$(cat "/proc/$1/stat" 2> /dev/null) || return 1
  process_state=${process_stat##*) }
  [ "${process_state%% *}" != Z ]
}

await_owner() {
  deadline=$(($(date +%s) + 120))
  until [ "$(owner)" = "$1" ]; do
    alive "$1" \
      || fail "the server $1 exited before it answered: $(tail -n 3 "$systemd_state/service.log")"
    [ "$(date +%s)" -lt "$deadline" ] || fail "the server $1 did not answer for the home within the hang guard"
    sleep 0.05
  done
}

await_exit() {
  deadline=$(($(date +%s) + 120))
  while alive "$1"; do
    [ "$(date +%s)" -lt "$deadline" ] || fail "the process $1 did not exit within the hang guard"
    sleep 0.05
  done
}

page() {
  url=$(on_machine "$quadratique" ui | jq -r .url)
  curl -fsS "$url" || fail "the installed binary does not serve $url"
}

title_of() {
  on_machine "$1" show --issue "$2" --json | jq -r .issue.title
}

build() {
  cp -R "$root/web/dist" "$scratch/dist"
  echo "<!-- build $1 -->" >> "$scratch/dist/index.html"
  cp "$(QUADRATIQUE_WEB_DIST=/nonexistent sh "$here/build.sh" "$scratch/dist" "$root/target/install-test")" "$scratch/quadratique-$1"
  cp "$scratch/dist/index.html" "$scratch/index-$1.html"
  rm -rf "$scratch/dist"
}

mkdir "$scratch/fake-cargo"
printf '#!/bin/sh\ntouch "%s"\n' "$scratch/cargo-called" > "$scratch/fake-cargo/cargo"
chmod +x "$scratch/fake-cargo/cargo"
if PATH="$scratch/fake-cargo:$PATH" sh "$here/build.sh" "$scratch/no-web-app" "$scratch/target" > /dev/null 2>&1; then
  fail "build.sh succeeds without a web app folder"
fi
[ ! -e "$scratch/cargo-called" ] || fail "build.sh calls cargo without a web app folder"
pass "build.sh fails before cargo when the web app folder is missing"

echo "building two release binaries from marked copies of web/dist, which are deleted before the binaries run"
install_build=$root/target/release/quadratique
install_build_before=$(sha256sum "$install_build" 2> /dev/null || echo none)
build old
build new
[ "$(sha256sum "$install_build" 2> /dev/null || echo none)" = "$install_build_before" ] \
  || fail "the test's build changed $install_build, which the install copies"
pass "the test builds into its own target folder, and an inherited QUADRATIQUE_WEB_DIST does not choose the web app"

target=$(rustc -vV | sed -n 's/^host: //p')
archive=$(sh "$here/package.sh" "$scratch/quadratique-new" 9.8.7 "$target" "$scratch/release")
[ "$archive" = "$scratch/release/quadratique-9.8.7-$target.tar.gz" ] || fail "package.sh names its archive $archive"
(cd "$scratch/release" && sha256sum -c --quiet "quadratique-9.8.7-$target.tar.gz.sha256") \
  || fail "the archive does not match its SHA-256 file"
[ "$(tar -tzf "$archive" | LC_ALL=C sort | tr '\n' ' ')" = "quadratique-9.8.7-$target/ quadratique-9.8.7-$target/LICENSE quadratique-9.8.7-$target/README.md quadratique-9.8.7-$target/install.sh quadratique-9.8.7-$target/quadratique quadratique-9.8.7-$target/uninstall.sh " ] \
  || fail "the archive holds $(tar -tzf "$archive" | tr '\n' ' ')"
mkdir "$scratch/download"
tar -xzf "$archive" -C "$scratch/download"
release=$scratch/download/quadratique-9.8.7-$target
cmp -s "$release/quadratique" "$scratch/quadratique-new" && [ -x "$release/quadratique" ] \
  || fail "the archive's quadratique is not the packaged binary, or is not executable"
pass "package.sh makes one archive named for the version and the target, with its SHA-256 file, the binary and the install scripts"

cd "$scratch"
sleep 600 &
unrelated=$!
echo "$unrelated" > "$systemd_state/pid"
echo 1 > "$systemd_state/started"
on_machine "$scratch/systemctl" --user stop quadratique.service
alive "$unrelated" || fail "the fake systemctl signalled a process that has the pid it recorded but not its start time"
kill "$unrelated"
pass "the fake systemctl signals only the process it started, not another with the same pid"
mkdir -p "$bin_home" "$(dirname "$unit")"
if on_machine env XDG_CONFIG_HOME="$home/elsewhere" sh "$here/install.sh" "$scratch/quadratique-old" > /dev/null 2> "$scratch/refused.log"; then
  fail "install.sh accepts an XDG_CONFIG_HOME whose unit folder the user manager may not search"
fi
grep -q XDG_CONFIG_HOME "$scratch/refused.log" || fail "install.sh does not say why it refuses: $(cat "$scratch/refused.log")"
[ ! -e "$home/elsewhere" ] && [ ! -e "$quadratique" ] || fail "install.sh wrote files before it refused"
pass "install.sh refuses an XDG_CONFIG_HOME other than ~/.config, and writes nothing"
for destination in "$quadratique" "$unit"; do
  mkdir "$destination"
  if on_machine sh "$here/install.sh" "$scratch/quadratique-old" > /dev/null 2> "$scratch/folder.log"; then
    fail "install.sh succeeds when $destination is a folder"
  fi
  [ -z "$(ls -A "$destination")" ] || fail "install.sh wrote into the folder $destination"
  [ -z "$(find "$bin_home" "$(dirname "$unit")" -name '.quadratique*')" ] || fail "install.sh left a staged file"
  rmdir "$destination"
done
[ ! -e "$quadratique" ] || fail "install.sh installed a binary although the unit's place is a folder"
pass "install.sh refuses a binary or unit destination that is a folder, before it writes anything"
echo "an unrelated file" > "$scratch/victim"
ln -s "$scratch/victim" "$quadratique.new"
ln -s "$scratch/victim" "$unit.new"
on_machine sh "$here/install.sh" "$scratch/quadratique-old" > /dev/null 2>> "$scratch/install.log" \
  || fail "install.sh fails: $(tail -n 5 "$scratch/install.log")"
[ "$(cat "$scratch/victim")" = "an unrelated file" ] && [ ! -x "$scratch/victim" ] \
  || fail "install.sh wrote through a link at a staging name"
pass "install.sh never writes through a file that already has a staging name"
[ -x "$quadratique" ] || fail "install.sh did not install $quadratique"
[ -f "$unit" ] || fail "install.sh did not write $unit"
[ "$(owner)" = "$(service)" ] || fail "install.sh returned before the service's server answered"
mkdir -p -m 700 "$home/run"
on_machine systemd-analyze verify --user "$unit" > "$scratch/verify.log" 2>&1 \
  || fail "systemd-analyze refuses $unit: $(cat "$scratch/verify.log")"
[ ! -s "$scratch/verify.log" ] || fail "systemd-analyze warns about $unit: $(cat "$scratch/verify.log")"
enabled_copy=$scratch/enabled-copy/systemd/user
mkdir -p "$enabled_copy/default.target.wants"
cp "$unit" "$enabled_copy/quadratique.service"
ln -s ../quadratique.service "$enabled_copy/default.target.wants/quadratique.service"
on_machine env XDG_CONFIG_HOME="$scratch/enabled-copy" /usr/lib/systemd/systemd --test --user --no-pager \
  > "$scratch/systemd-test.log" 2> /dev/null || true
awk '/→ Unit quadratique\.service:/{found=1; print; next} found && /→ Unit /{exit} found{print}' \
  "$scratch/systemd-test.log" > "$scratch/parsed-unit.log"
for expected in "Environment: XDG_DATA_HOME=$data_home" "Command Line: \"$quadratique\" serve --take-over" \
  "Restart: on-failure" "RestartSec: 2s" "TimeoutStopSec: 1min 10s"; do
  grep -qF "$expected" "$scratch/parsed-unit.log" || fail "systemd does not read \"$expected\" from $unit"
done
pass "systemd reads the unit's quoted paths whole and its restart policy, with spaces in the binary and data folders"

damaged_home=$scratch/damaged
mkdir -p "$damaged_home"
echo "notes, not a database" > "$damaged_home/workspace.db"
refused_status=0
on_machine env QUADRATIQUE_HOME="$damaged_home" "$quadratique" serve > /dev/null 2>&1 || refused_status=$?
[ "$refused_status" != 0 ] && grep -qx "RestartPreventExitStatus=$refused_status" "$unit" \
  || fail "the unit restarts a server that exits with status $refused_status on a store it cannot open"
pass "the unit does not restart a server that exits because its store cannot open (status $refused_status)"

page > "$scratch/page.html"
cmp -s "$scratch/page.html" "$scratch/index-old.html" || fail "the page is not the index.html built into the binary"
pass "the installed binary serves the web app's index page, with the folder it was built from deleted"

on_machine "$scratch/systemctl" --user stop quadratique.service
on_machine "$quadratique" ui > /dev/null
command_started=$(owner)
on_machine "$scratch/systemctl" --user restart quadratique.service
await_owner "$(service)"
[ "$(service)" != "$command_started" ] || fail "the service's server is the command's"
await_servers 1 "after the take-over"
pass "the service takes over the home from a server that a command started"

on_machine "$quadratique" ui > /dev/null
[ "$(owner)" = "$(service)" ] || fail "a command left the service's server $(service)"
[ "$(servers)" = 1 ] || fail "$(servers) servers run for the home after a command, not 1"
pass "a command uses the service's server and starts no other"

on_machine "$quadratique" project create --key KEEP --name Kept > /dev/null
on_machine "$quadratique" create --project KEEP --title "Before the upgrade" --acceptance "It is kept." > /dev/null
old=$(service)
rm -f "$scratch/cargo-called"
on_machine env PATH="$scratch/fake-cargo:/usr/bin:/bin" sh "$release/install.sh" "$release/quadratique" > /dev/null 2>> "$scratch/install.log" \
  || fail "the archive's install.sh fails over a running server"
[ ! -e "$scratch/cargo-called" ] || fail "the install from the archive called cargo"
[ "$(owner)" = "$(service)" ] || fail "install.sh returned before the new server answered"
[ "$(service)" != "$old" ] || fail "the old server $old still owns the home after the upgrade"
[ "$(readlink "/proc/$(service)/exe")" = "$quadratique" ] || fail "the new server does not run $quadratique"
page > "$scratch/page.html"
cmp -s "$scratch/page.html" "$scratch/index-new.html" || fail "the upgraded server does not serve the new build's page"
[ "$(title_of "$quadratique" KEEP-1)" = "Before the upgrade" ] || fail "KEEP-1 is not in the store after the upgrade"
await_servers 1 "after the upgrade"
pass "an install from a release archive, with no Rust toolchain on PATH, over a running old build ends with the new build answering on the same store"

crashed=$(service)
kill -KILL "$crashed"
await_exit "$crashed"
on_machine "$scratch/systemctl" --user restart quadratique.service
await_owner "$(service)"
pass "after the server is killed, the unit's command takes the home again"

install_must_fail() {
  if on_machine sh "$here/install.sh" "$scratch/quadratique-new" > /dev/null 2> "$scratch/not-ready.log"; then
    fail "install.sh succeeds when $1"
  fi
  grep -q "fake: " "$scratch/not-ready.log" || fail "install.sh does not show the unit's status when $1: $(cat "$scratch/not-ready.log")"
}

touch "$systemd_state/restart-does-nothing"
install_must_fail "the restart leaves the old server running its deleted binary"
rm "$systemd_state/restart-does-nothing"
pass "an install whose restart leaves the old server fails and shows the unit's status"

on_machine "$scratch/systemctl" --user stop quadratique.service
on_machine "$quadratique" ui > /dev/null
touch "$systemd_state/broken"
install_must_fail "the service starts nothing while a command's server answers"
rm "$systemd_state/broken"
pass "an install whose service starts no server fails, even while a command's server answers"

touch "$systemd_state/restart-fails"
install_must_fail "systemctl restart fails"
rm "$systemd_state/restart-fails"
pass "an install whose systemctl restart fails shows the unit's status"

on_machine "$scratch/systemctl" --user restart quadratique.service
await_owner "$(service)"

on_machine "$quadratique" create --project KEEP --title "Before the uninstall" --acceptance "It is kept." > /dev/null

on_machine "$scratch/systemctl" --user stop quadratique.service
on_machine "$quadratique" ui > /dev/null
frozen=$(owner)
kill -STOP "$frozen"
if on_machine sh "$here/uninstall.sh" > /dev/null 2> "$scratch/uninstall.log"; then
  fail "uninstall.sh succeeds while a server it could not stop holds the home"
fi
[ -e "$quadratique" ] && [ -e "$unit" ] || fail "uninstall.sh removed files although a server still holds the home"
[ -e "$systemd_state/enabled" ] || fail "uninstall.sh disabled the unit although a server still holds the home"
kill -KILL "$frozen"
kill -CONT "$frozen"
await_exit "$frozen"
pass "uninstall.sh fails, and neither disables nor removes anything, when a server of the home does not stop"

on_machine "$quadratique" ui > /dev/null
orphan=$(owner)
rm "$quadratique"
if on_machine sh "$here/uninstall.sh" > /dev/null 2> "$scratch/uninstall.log"; then
  fail "uninstall.sh succeeds without its binary while a server holds the home"
fi
grep -q "missing" "$scratch/uninstall.log" || fail "uninstall.sh does not say what it could not check: $(cat "$scratch/uninstall.log")"
[ -e "$unit" ] && [ -e "$systemd_state/enabled" ] || fail "uninstall.sh removed the unit although a server holds the home"
kill -KILL "$orphan"
await_exit "$orphan"
cp "$scratch/quadratique-new" "$quadratique"
pass "uninstall.sh without its binary says what it could not check, and fails while a server holds the home"

on_machine "$scratch/systemctl" --user restart quadratique.service
await_owner "$(service)"
on_machine env XDG_DATA_HOME="$home/other" sh "$release/uninstall.sh" > /dev/null
[ ! -e "$home/other" ] || fail "uninstall.sh wrote into the XDG_DATA_HOME of its own shell"
[ ! -e "$quadratique" ] || fail "uninstall.sh left $quadratique"
[ ! -e "$unit" ] || fail "uninstall.sh left $unit"
await_servers 0 "after uninstall.sh"
[ "$(title_of "$scratch/quadratique-new" KEEP-1)" = "Before the upgrade" ] || fail "KEEP-1 is gone after the uninstall"
[ "$(title_of "$scratch/quadratique-new" KEEP-2)" = "Before the uninstall" ] || fail "KEEP-2 is gone after the uninstall"
on_machine "$scratch/quadratique-new" server stop > /dev/null
pass "uninstall removes the binary and the unit, stops the server, and the store keeps every issue"
