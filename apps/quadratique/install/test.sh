#!/bin/sh
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
scratch=$(mktemp -d)
case $scratch in *[[:space:]]*) echo "the temporary folder $scratch has a space" >&2; exit 1 ;; esac
home=$scratch/home
bin_home="$home/my bin"
quadratique=$bin_home/quadratique
# The server's socket is in the data folder, and its path may be at most 103 bytes on macOS: the data folder is short.
data_parent=$(mktemp -d /tmp/qinstall.XXXXXX)
data=$data_parent/home
started=""

cleanup() {
  for binary in "$quadratique" "$scratch/quadratique-new" "$scratch/quadratique-old"; do
    [ -x "$binary" ] && on_machine "$binary" server stop > /dev/null 2>&1 || true
  done
  for pid in $started; do kill -KILL "$pid" 2> /dev/null || true; done
  rm -rf "$scratch" "$data_parent"
}
trap cleanup EXIT INT TERM

show_file() {
  echo "--- $1" >&2
  if [ -f "$1" ]; then cat "$1" >&2; else echo "(none)" >&2; fi
}

fail() {
  echo "FAIL: $*" >&2
  show_file "$data/server.log"
  show_file "$scratch/install.log"
  show_file "$scratch/serve.log"
  exit 1
}

pass() {
  echo "ok: $*"
}

mkdir -p "$home"

exec_on_machine() {
  exec env -i PATH="$PATH" HOME="$home" USER=tester XDG_BIN_HOME="$bin_home" QUADRATIQUE_HOME="$data" \
    XDG_RUNTIME_DIR="$home/run" "$@"
}

on_machine() {
  (exec_on_machine "$@")
}

# The fields that the readiness check of an earlier install and upgrade reads (record 0004, §6): the server answers
# for its home, of this build, and names its own process.
owner() {
  on_machine "$1" server status | jq -r 'select(.running and .sameInterface) | .pid'
}

# A process that exited and waits to be reaped (Z) counts as gone.
alive() {
  state=$(ps -o stat= -p "$1" 2> /dev/null) || return 1
  case $state in Z*) return 1 ;; *) return 0 ;; esac
}

await_owner() {
  deadline=$(($(date +%s) + 120))
  until [ "$(owner "$2")" = "$1" ]; do
    alive "$1" || fail "the process $1 exited before it answered: $(tail -n 3 "$scratch/serve.log" 2> /dev/null)"
    [ "$(date +%s)" -lt "$deadline" ] || fail "the process $1 did not answer for the home within the hang guard"
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

# Every file under the home, except the installed binary's folder.
outside_the_install() {
  (cd "$home" && find . -path "./my bin" -prune -o -print | LC_ALL=C sort)
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
install_build_before=$(cksum "$install_build" 2> /dev/null || echo none)
build old
build new
[ "$(cksum "$install_build" 2> /dev/null || echo none)" = "$install_build_before" ] \
  || fail "the test's build changed $install_build, which the install copies"
pass "the test builds into its own target folder, and an inherited QUADRATIQUE_WEB_DIST does not choose the web app"

# The release archive, made with GNU tar and sha256sum on Linux, with bsdtar and shasum on macOS.
if command -v sha256sum > /dev/null; then sha256="sha256sum"; else sha256="shasum -a 256"; fi
target=$(rustc -vV | sed -n 's/^host: //p')
archive=$(sh "$here/package.sh" "$scratch/quadratique-new" 9.8.7 "$target" "$scratch/release")
[ "$archive" = "$scratch/release/quadratique-9.8.7-$target.tar.gz" ] || fail "package.sh names its archive $archive"
(cd "$scratch/release" && $sha256 -c "quadratique-9.8.7-$target.tar.gz.sha256" > /dev/null) \
  || fail "the archive does not match its SHA-256 file"
[ "$(tar -tzf "$archive" | LC_ALL=C sort | tr '\n' ' ')" = "quadratique-9.8.7-$target/ quadratique-9.8.7-$target/LICENSE quadratique-9.8.7-$target/README.md quadratique-9.8.7-$target/bin/ quadratique-9.8.7-$target/bin/quadratique quadratique-9.8.7-$target/install.sh " ] \
  || fail "the archive holds $(tar -tzf "$archive" | tr '\n' ' ')"
mkdir "$scratch/download"
tar -xzf "$archive" -C "$scratch/download"
release=$scratch/download/quadratique-9.8.7-$target
cmp -s "$release/bin/quadratique" "$scratch/quadratique-new" && [ -x "$release/bin/quadratique" ] \
  || fail "the archive's quadratique is not the packaged binary, or is not executable"
pass "package.sh makes one archive named for the version and the target, with its SHA-256 file, the binary alone in bin/ and install.sh"

cd "$scratch"
before_install=$(outside_the_install)
mkdir -p "$quadratique"
if on_machine sh "$here/install.sh" "$scratch/quadratique-old" > /dev/null 2> "$scratch/folder.log"; then
  fail "install.sh succeeds when $quadratique is a folder"
fi
[ -z "$(ls -A "$quadratique")" ] || fail "install.sh wrote into the folder $quadratique"
[ -z "$(find "$bin_home" -name '.quadratique*')" ] || fail "install.sh left a staged file"
rmdir "$quadratique"
pass "install.sh refuses a destination that is a folder, before it writes anything"

echo "an unrelated file" > "$scratch/victim"
ln -s "$scratch/victim" "$quadratique.new"
on_machine sh "$here/install.sh" "$scratch/quadratique-old" > /dev/null 2>> "$scratch/install.log" \
  || fail "install.sh fails: $(tail -n 5 "$scratch/install.log")"
[ "$(cat "$scratch/victim")" = "an unrelated file" ] && [ ! -x "$scratch/victim" ] \
  || fail "install.sh wrote through a link at a staging name"
rm "$quadratique.new"
pass "install.sh never writes through a file that already has a staging name"

relative_home=$scratch/relative-home
mkdir -p "$relative_home"
(cd "$relative_home" && env -i PATH="$PATH" HOME="$relative_home" XDG_BIN_HOME=bin sh "$here/install.sh" "$scratch/quadratique-old" > /dev/null 2>&1) \
  || fail "install.sh fails with a relative XDG_BIN_HOME"
[ -x "$relative_home/.local/bin/quadratique" ] && [ ! -e "$relative_home/bin" ] \
  || fail "install.sh used a relative XDG_BIN_HOME, which the XDG specification says to ignore"
pass "install.sh ignores a relative XDG_BIN_HOME and installs in ~/.local/bin"

locked_home=$scratch/locked-home
mkdir -p "$locked_home/bin"
chmod 555 "$locked_home/bin"
if env -i PATH="$PATH" HOME="$locked_home" XDG_BIN_HOME="$locked_home/bin" sh "$here/install.sh" "$scratch/quadratique-old" > /dev/null 2> "$scratch/locked.log"; then
  fail "install.sh succeeds in a folder it cannot write"
fi
chmod 755 "$locked_home/bin"
grep -q "^install.sh: cannot write in $locked_home/bin" "$scratch/locked.log" && [ "$(wc -l < "$scratch/locked.log")" -eq 1 ] \
  || fail "install.sh does not say in its own words what it could not do: $(cat "$scratch/locked.log")"
if env -i PATH="$PATH" HOME="$locked_home" sh "$here/install.sh" "$scratch/no-such-binary" > /dev/null 2> "$scratch/missing.log"; then
  fail "install.sh succeeds without a binary"
fi
grep -q "^install.sh: $scratch/no-such-binary is not a file" "$scratch/missing.log" && [ "$(wc -l < "$scratch/missing.log")" -eq 1 ] \
  || fail "install.sh does not say in its own words that the binary is missing: $(cat "$scratch/missing.log")"
pass "install.sh says in its own words what it could not do, and what to do"
[ -x "$quadratique" ] && cmp -s "$quadratique" "$scratch/quadratique-old" || fail "install.sh did not copy the binary to $quadratique"
[ "$(outside_the_install)" = "$before_install" ] || fail "install.sh wrote outside $bin_home: $(outside_the_install)"
[ -z "$(owner "$quadratique" 2> /dev/null)" ] || fail "install.sh started a server"
pass "install.sh copies the binary to a folder with a space in its path, writes nothing else and starts nothing"

page > "$scratch/page.html"
cmp -s "$scratch/page.html" "$scratch/index-old.html" || fail "the page is not the index.html built into the binary"
pass "the installed binary serves the web app's index page, with the folder it was built from deleted"

on_machine "$quadratique" server stop > /dev/null
(exec_on_machine "$quadratique" serve --take-over < /dev/null >> "$scratch/serve.log" 2>&1) &
supervised=$!
started="$started $supervised"
await_owner "$supervised" "$quadratique"
pass "serve --take-over with no server running serves the home as its own main process, and server status says running, sameInterface and its pid"

on_machine "$quadratique" project create --key KEEP --name Kept > /dev/null
on_machine "$quadratique" create --project KEEP --title "Before the upgrade" --acceptance "It is kept." > /dev/null
rm -f "$scratch/cargo-called"
on_machine env PATH="$scratch/fake-cargo:/usr/bin:/bin" sh "$release/install.sh" "$release/bin/quadratique" > "$scratch/over.log" 2>> "$scratch/install.log" \
  || fail "the release's install.sh fails over a running server"
grep -q "keeps running the earlier build until \`quadratique server stop\`" "$scratch/over.log" \
  || fail "install.sh does not say that the running server keeps the earlier build: $(cat "$scratch/over.log")"
[ ! -e "$scratch/cargo-called" ] || fail "the install from the release called cargo"
cmp -s "$quadratique" "$scratch/quadratique-new" || fail "the release's install.sh did not replace $quadratique"
[ "$(owner "$quadratique")" = "$supervised" ] || fail "the install changed the server that runs"
pass "an install from a release, with no Rust toolchain on PATH, replaces the binary, leaves the running server alone and says that it keeps the earlier build until server stop"

(exec_on_machine "$quadratique" serve --take-over < /dev/null >> "$scratch/serve.log" 2>&1) &
taken_over_by=$!
started="$started $taken_over_by"
await_owner "$taken_over_by" "$quadratique"
await_exit "$supervised"
page > "$scratch/page.html"
cmp -s "$scratch/page.html" "$scratch/index-new.html" || fail "the new build's server does not serve the new build's page"
[ "$(title_of "$quadratique" KEEP-1)" = "Before the upgrade" ] || fail "KEEP-1 is not in the store after the take-over"
pass "serve --take-over of the new binary stops the old build's server and serves the same store as its own main process"

on_machine "$quadratique" server stop > /dev/null
await_exit "$taken_over_by"
on_machine "$quadratique" show --issue KEEP-1 > /dev/null
[ "$(title_of "$quadratique" KEEP-1)" = "Before the upgrade" ] || fail "KEEP-1 is gone after the stop"
on_machine "$quadratique" server stop > /dev/null
pass "after the supervised server stops, the next command starts the new build on the same store"
