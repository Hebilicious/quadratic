#!/bin/sh
# Checks the packaging preflight and how build.rs embeds the web app, in the repository and in a packaged crate.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
repository=$(cd "$root/../.." && pwd)
scratch=$(cd "$(mktemp -d)" && pwd -P)
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

# The test stages its own copy and must leave a web app that a maintainer staged in this checkout as it was.
staged_here() { find "$root/server/web-dist" -type f -exec sha256sum {} + 2> /dev/null | LC_ALL=C sort; }
staged_before=$(staged_here)

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

# build.rs, read from cargo's report of the build script: `embedding` prints `false` when it embeds nothing, else the
# folder it embeds, and fails when cargo or the report fails.
embedding() {
  manifest=$1
  shift
  if ! CARGO_TARGET_DIR=$target cargo check --quiet --locked --manifest-path "$manifest" --message-format=json "$@" \
    2> "$scratch/check.log" > "$scratch/check.json"; then
    echo "cargo check failed" >> "$scratch/check.log"
    return 1
  fi
  report=$(jq -c -s 'map(select(.reason == "build-script-executed" and (.package_id | contains("quadratique-server")))) | last' \
    "$scratch/check.json") || return 1
  [ "$report" != null ] || { echo "cargo reported no build script of quadratique-server" >> "$scratch/check.log"; return 1; }
  if [ "$(printf '%s' "$report" | jq -r '.cfgs | index("embedded_web") != null')" = false ]; then
    echo false
    return 0
  fi
  generated=$(printf '%s' "$report" | jq -r .out_dir)/web_app.rs
  [ -f "$generated" ] || { echo "the build script set embedded_web but wrote no $generated" >> "$scratch/check.log"; return 1; }
  sed -n -E 's/.*include_bytes!\("(.*)(\/|\\\\)index\.html"\).*/\1/p' "$generated"
}

# A folder as the comparison needs it. On Windows (Git's bash, which has cygpath) build.rs writes the canonical Windows
# path, `\\?\D:\a\...` with each backslash doubled, and the shell knows the folder as `/d/a/...`: both become `D:/a/...`.
comparable() {
  if [ "$1" = false ] || ! command -v cygpath > /dev/null; then
    printf '%s\n' "$1"
    return
  fi
  cygpath -m -l "$(printf '%s' "$1" | sed 's/\\\\/\\/g; s/^\\\\?\\//')"
}

expect_embedding() {
  wanted=$1
  what=$2
  shift 2
  got=$(embedding "$@") || fail "$what: the build fails: $(tail -n 5 "$scratch/check.log")"
  [ "$(comparable "$got")" = "$(comparable "$wanted")" ] || fail "$what: it embeds ${got:-a folder with no index.html}, not $wanted"
  pass "$what"
}

expect_refusal() {
  what=$1
  shift
  if got=$(embedding "$@"); then fail "$what: the build succeeds and embeds $got"; fi
  grep -q "packaged without its web app" "$scratch/check.log" \
    || fail "$what: the build fails without saying to stage the web app: $(tail -n 5 "$scratch/check.log")"
  pass "$what"
}

other=$scratch/other-web-app
cp -R "$root/web/dist" "$other"
echo "<!-- another build -->" >> "$other/index.html"

server_manifest=$root/server/Cargo.toml
expect_embedding false "a build in the repository without the feature embeds nothing and reads web/dist at each request" "$server_manifest"
expect_embedding "$(cd "$root/web/dist" && pwd -P)" "a build in the repository with the feature embedded-web embeds web/dist" \
  "$server_manifest" --features embedded-web
QUADRATIQUE_WEB_DIST=$other expect_embedding "$other" "a build in the repository with the feature embeds the folder that QUADRATIQUE_WEB_DIST names" \
  "$server_manifest" --features embedded-web

# A copy of the workspace, staged as a publication stages it, packaged without touching this checkout's server/web-dist.
workspace=$scratch/workspace
mkdir -p "$workspace/apps/quadratique"
cp "$repository/LICENSE" "$workspace/LICENSE"
# The targets of the crates' links first: Git's bash on Windows copies a link's target, which must exist by then.
# The catalog's node_modules is not part of a crate, and its links are junctions that Git's bash cannot copy.
mkdir "$workspace/apps/quadratique/catalog"
for entry in "$root/catalog"/*; do
  [ "${entry##*/}" = node_modules ] || cp -a "$entry" "$workspace/apps/quadratique/catalog/"
done
for path in Cargo.toml Cargo.lock rust-toolchain.toml README.md skill server cli; do
  cp -a "$root/$path" "$workspace/apps/quadratique/$path"
done
rm -rf "$workspace/apps/quadratique/server/web-dist"
cp -R "$root/web/dist" "$workspace/apps/quadratique/server/web-dist"
(cd "$workspace/apps/quadratique" && CARGO_TARGET_DIR=$target cargo package --quiet --locked -p quadratique-server --no-verify) \
  || fail "cargo package refuses the server crate"
mkdir "$scratch/registry"
tar -xzf "$target/package/quadratique-server-$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml").crate" -C "$scratch/registry"
packaged=$(find "$scratch/registry" -mindepth 1 -maxdepth 1 -type d)
# A .crate keeps fixed file times, and cargo's hash of a path crate leaves out its folder, so without this the shared
# target folder would reuse a build script compiled from an earlier package.
find "$packaged" -exec touch {} +
[ ! -e "$packaged/moon.yml" ] || fail "the packaged crate carries moon.yml, the repository's marker"
staged=$packaged/web-dist

expect_embedding "$staged" "a packaged crate embeds the web app it carries, without the feature" "$packaged/Cargo.toml"
mkdir -p "$scratch/registry/web/dist"
echo "an unrelated folder" > "$scratch/registry/web/dist/notes.txt"
expect_embedding "$staged" "a packaged crate next to an unrelated folder named web still embeds the web app it carries" "$packaged/Cargo.toml"
expect_embedding "$staged" "a packaged crate with the feature embedded-web embeds the web app it carries" \
  "$packaged/Cargo.toml" --features embedded-web
QUADRATIQUE_WEB_DIST=$other expect_embedding "$staged" "a packaged crate ignores QUADRATIQUE_WEB_DIST and embeds the web app it carries" \
  "$packaged/Cargo.toml" --features embedded-web

rm -rf "$staged"
expect_refusal "a packaged crate with no web app fails to build and says to stage it" "$packaged/Cargo.toml"
QUADRATIQUE_WEB_DIST=$other expect_refusal "a packaged crate with no web app fails even when QUADRATIQUE_WEB_DIST names one" \
  "$packaged/Cargo.toml" --features embedded-web

[ "$(staged_here)" = "$staged_before" ] || fail "the test changed the web app staged in $root/server/web-dist"
pass "the test leaves the web app staged in this checkout as it was"
