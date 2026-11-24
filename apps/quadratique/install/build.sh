#!/bin/sh
set -eu
dist=${1:?usage: build.sh <web app folder> <cargo target folder>}
target=${2:?usage: build.sh <web app folder> <cargo target folder>}
root=$(cd "$(dirname "$0")/.." && pwd)
if [ ! -f "$dist/index.html" ]; then
  echo "build.sh: $dist has no index.html; run \`moon run quadratique-web:build\`" >&2
  exit 1
fi
web_app=$(cd "$dist" && pwd)
QUADRATIQUE_WEB_DIST=$web_app cargo build --quiet --release -p quadratique --features embedded-web \
  --manifest-path "$root/Cargo.toml" --target-dir "$target"
echo "$target/release/quadratique"
