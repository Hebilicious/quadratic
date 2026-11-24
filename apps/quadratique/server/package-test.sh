#!/bin/sh
# Checks the packaging preflight and how build.rs embeds the web app, in the repository and in a packaged crate.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
repository=$(cd "$root/../.." && pwd)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
target=$root/target/package-test

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

pass() {
  echo "ok: $*"
}

[ -f "$root/web/dist/index.html" ] || fail "$root/web/dist has no index.html: run moon run quadratique-web:build"

# A copy of what the preflight reads, links kept as links.
fixture() {
  rm -rf "$scratch/repo"
  mkdir -p "$scratch/repo/apps/quadratique"
  cp "$repository/LICENSE" "$scratch/repo/LICENSE"
  for path in catalog/quadratique.acidcat.json skill/quadratique server/src/app/quadratique.acidcat.json \
    cli/src/skill server/LICENSE cli/LICENSE web/dist server/preflight.sh; do
    mkdir -p "$scratch/repo/apps/quadratique/$(dirname "$path")"
    cp -a "$root/$path" "$scratch/repo/apps/quadratique/$path"
  done
  cp -R "$root/web/dist" "$scratch/repo/apps/quadratique/server/web-dist"
  fixture=$scratch/repo/apps/quadratique
}

preflight_refuses() {
  if sh "$fixture/server/preflight.sh" "$fixture" > /dev/null 2> "$scratch/preflight.log"; then
    fail "the preflight accepts $1"
  fi
  grep -q "$2" "$scratch/preflight.log" || fail "the preflight refuses $1 without saying \"$2\": $(cat "$scratch/preflight.log")"
  pass "the preflight refuses $1 and says why"
}

fixture
sh "$fixture/server/preflight.sh" "$fixture" > /dev/null 2> "$scratch/preflight.log" \
  || fail "the preflight refuses a complete checkout: $(cat "$scratch/preflight.log")"
pass "the preflight accepts a checkout with the staged web app, the links and the licence"

fixture
rm -rf "$fixture/server/web-dist"
preflight_refuses "a server crate with no staged web app" "quadratique-server:stage-web"

fixture
echo "<!-- an older build -->" >> "$fixture/server/web-dist/index.html"
preflight_refuses "a staged web app that is not the current build" "is not the web app in"

fixture
rm "$fixture/server/src/app/quadratique.acidcat.json"
printf '%s' ../../../catalog/quadratique.acidcat.json > "$fixture/server/src/app/quadratique.acidcat.json"
preflight_refuses "a link written as a plain file by a checkout without symbolic links" "no symbolic links"

fixture
rm "$fixture/cli/src/skill/planning.md"
preflight_refuses "a missing skill file" "planning.md is missing"

fixture
rm "$fixture/cli/LICENSE"
echo "another licence" > "$fixture/cli/LICENSE"
preflight_refuses "a licence that is not the repository's" "cli/LICENSE is not the same"

# build.rs: whether the cfg embedded_web is set, read from cargo's report of the build script.
embeds() {
  manifest=$1
  shift
  CARGO_TARGET_DIR=$target cargo check --quiet --locked --manifest-path "$manifest" --message-format=json "$@" \
    2> "$scratch/check.log" > "$scratch/check.json" || return 2
  jq -e -s 'map(select(.reason == "build-script-executed" and (.package_id | contains("quadratique-server")))) | last | .cfgs | index("embedded_web") != null' \
    "$scratch/check.json" > /dev/null
}

server_manifest=$root/server/Cargo.toml
if embeds "$server_manifest"; then fail "a build in the repository without the feature embeds the web app"; fi
pass "a build in the repository without the feature embeds nothing and reads web/dist at each request"
embeds "$server_manifest" --features embedded-web || fail "a build in the repository with embedded-web does not embed web/dist: $(cat "$scratch/check.log")"
pass "a build in the repository with the feature embedded-web embeds web/dist"

rm -rf "$root/server/web-dist"
cp -R "$root/web/dist" "$root/server/web-dist"
trap 'rm -rf "$scratch" "$root/server/web-dist"' EXIT
(cd "$root" && CARGO_TARGET_DIR=$target cargo package --quiet --locked -p quadratique-server --allow-dirty --no-verify) \
  || fail "cargo package refuses the server crate"
rm -rf "$root/server/web-dist"
mkdir "$scratch/registry"
tar -xzf "$target/package/quadratique-server-$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml").crate" -C "$scratch/registry"
packaged=$(find "$scratch/registry" -mindepth 1 -maxdepth 1 -type d)
[ ! -e "$packaged/moon.yml" ] || fail "the packaged crate carries moon.yml, the repository's marker"

embeds "$packaged/Cargo.toml" || fail "a packaged crate does not embed its web-dist: $(cat "$scratch/check.log")"
pass "a packaged crate embeds the web app it carries, without the feature"

mkdir -p "$scratch/registry/web/dist"
echo "an unrelated folder" > "$scratch/registry/web/dist/notes.txt"
embeds "$packaged/Cargo.toml" || fail "a packaged crate next to an unrelated web folder does not embed its web-dist: $(cat "$scratch/check.log")"
pass "a packaged crate next to an unrelated folder named web still embeds the web app it carries"

embeds "$packaged/Cargo.toml" --features embedded-web || fail "a packaged crate with embedded-web does not embed its web-dist: $(cat "$scratch/check.log")"
pass "a packaged crate with the feature embedded-web embeds the web app it carries"

rm -rf "$packaged/web-dist"
status=0
embeds "$packaged/Cargo.toml" || status=$?
[ "$status" = 2 ] || fail "a packaged crate with no web-dist builds (status $status)"
grep -q "packaged without its web app" "$scratch/check.log" || fail "a packaged crate with no web-dist fails without saying why: $(tail -n 5 "$scratch/check.log")"
pass "a packaged crate with no web app fails to build and says to stage it"
