#!/bin/sh
# Checks, before `cargo publish`, that both crates carry what they read at build time: the staged web app, the
# catalog JSON, the skill files and the licence. Run from anywhere; it reads the folder above this script.
set -eu
root=$(cd "${1:-$(dirname "$0")/..}" && pwd)
repository=$(cd "$root/../.." && pwd)
failures=0

refuse() {
  echo "preflight: $*" >&2
  failures=$((failures + 1))
}

# A crate's copy of a repository file is a link to it; cargo package copies the content it points to. A checkout
# without symlink support writes the link as a plain file that holds the path instead.
same_content() {
  copy=$1
  original=$2
  if [ ! -e "$copy" ] && [ ! -L "$copy" ]; then
    refuse "$copy is missing; it must be a link to $original"
  elif ! cmp -s "$copy" "$original"; then
    if [ ! -L "$copy" ] && [ "$(wc -l < "$copy")" -le 1 ] && grep -q '^\.\./' "$copy"; then
      refuse "$copy is a plain file that holds a path, not a link: this checkout has no symbolic links (git config core.symlinks is false). Check the repository out again with symbolic links."
    else
      refuse "$copy is not the same as $original"
    fi
  fi
}

same_content "$root/server/src/app/quadratique.acidcat.json" "$root/catalog/quadratique.acidcat.json"
same_content "$root/cli/src/skill/SKILL.md" "$root/skill/quadratique/SKILL.md"
same_content "$root/cli/src/skill/planning.md" "$root/skill/quadratique/planning.md"
same_content "$root/server/LICENSE" "$repository/LICENSE"
same_content "$root/cli/LICENSE" "$repository/LICENSE"

staged=$root/server/web-dist
built=$root/web/dist
if [ ! -f "$built/index.html" ]; then
  refuse "$built has no index.html: run moon run quadratique-web:build"
elif [ ! -f "$staged/index.html" ]; then
  refuse "$staged has no index.html: run moon run quadratique-server:stage-web"
elif ! diff -r "$staged" "$built" > /dev/null; then
  refuse "$staged is not the web app in $built: run moon run quadratique-server:stage-web again"
fi

if [ "$failures" -gt 0 ]; then
  echo "preflight: $failures problem(s); nothing is ready to publish." >&2
  exit 1
fi
echo "preflight: both crates carry the web app, the catalog JSON, the skill files and the licence."
