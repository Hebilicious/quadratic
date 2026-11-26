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
mkdir -p "$target"
target_dir=$(cd "$target" && pwd)
# From the workspace's folder, so that rustup uses the toolchain that its rust-toolchain.toml pins, whatever the
# caller's folder: from elsewhere it would take the machine's default toolchain.
(cd "$root" && QUADRATIQUE_WEB_DIST=$web_app cargo build --quiet --release -p quadratique --features embedded-web \
  --target-dir "$target_dir")
echo "$target/release/quadratique"
