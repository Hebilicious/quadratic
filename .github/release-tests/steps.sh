#!/bin/bash
# Runs the release workflow's own step scripts against a fake `gh` (./gh) and a folder of artifacts, case by case.
# Needs bash, jq, sha256sum and uv. Run: bash .github/release-tests/steps.sh
set -u
here=$(cd "$(dirname "$0")" && pwd)
workflow=${WORKFLOW:-$here/../workflows/release.yml}
# Inside the repository, so that the tools pinned in .prototools resolve in every case's folder.
mkdir -p "$here/../../.tmp"
work=$(mktemp -d "$here/../../.tmp/release-tests.XXXXXX")
trap 'rm -rf "$work"' EXIT
step() { uv run --quiet --with pyyaml==6.0.3 python -I "$here/run_step.py" "$workflow" "$@"; }
export PATH="$here:$PATH" GITHUB_REPOSITORY=Hebilicious/quadratique STEP_GH_TOKEN=fake
CHECK="Check that the four files are there and that each archive matches its SHA-256 file"
ATTACH="Attach the files, check the release, and publish it"
NAME="Name the release to finish"
MERGE=1111111111111111111111111111111111111111
LATER=2222222222222222222222222222222222222222
OTHER=3333333333333333333333333333333333333333
failures=0
check() {
  if [ "$1" = "$2" ]; then echo "ok: $3"; else echo "FAIL: $3 (got $1, wanted $2)"; failures=$((failures + 1)); fi
}

# A repository with the merged release commit $MERGE (manifest 0.1.0), a later commit $LATER, and a draft release
# v0.1.0 (id 7) whose tag points at $MERGE; the four built files are in $work/assets.
fresh() {
  rm -rf "$work/case" && mkdir -p "$work/case/assets" "$work/case/gh/assets"
  cd "$work/case/assets" || exit 1
  for target in aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu; do
    echo "archive for $target" > "quadratique-0.1.0-$target.tar.gz"
    sha256sum "quadratique-0.1.0-$target.tar.gz" > "quadratique-0.1.0-$target.tar.gz.sha256"
  done
  cat > "$work/case/gh/state.json" <<JSON
{"repo": "Hebilicious/quadratique",
 "releases": [{"id": 7, "tag_name": "v0.1.0", "draft": true, "target_commitish": "$MERGE", "assets": []}],
 "tags": {"v0.1.0": "$MERGE"},
 "manifests": {"$MERGE": "0.1.0", "$LATER": "0.1.0", "$OTHER": "0.0.9"},
 "compare": {"$MERGE...$LATER": "ahead", "$MERGE...$MERGE": "identical"}}
JSON
  export FAKE_GH=$work/case/gh STEP_VERSION=0.1.0 STEP_TAG=v0.1.0 STEP_RELEASE=7 STEP_SHA=$MERGE
  cd "$work/case" || exit 1
}
edit() { python3 -c "import json,sys; p='$work/case/gh/state.json'; s=json.load(open(p)); exec(sys.argv[1]); json.dump(s, open(p,'w'))" "$1"; }
draft() { python3 -c "import json; print(json.load(open('$work/case/gh/state.json'))['releases'][0]['draft'])"; }
attached() { python3 -c "import json; print(' '.join(sorted(a['name'] for a in json.load(open('$work/case/gh/state.json'))['releases'][0]['assets'])))"; }
changes() { grep -cE -- '-X (DELETE|POST|PATCH)' "$work/case/gh/calls" 2> /dev/null || true; }
attach_all() { (cd assets && for f in *; do gh api -X POST "https://uploads.github.com/repos/Hebilicious/quadratique/releases/7/assets?name=$f" -H "Content-Type: application/octet-stream" --input "$f" > /dev/null; done); : > gh/calls; }
four="quadratique-0.1.0-aarch64-unknown-linux-gnu.tar.gz quadratique-0.1.0-aarch64-unknown-linux-gnu.tar.gz.sha256 quadratique-0.1.0-x86_64-unknown-linux-gnu.tar.gz quadratique-0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256"
name_release() { out=$work/case/output; : > "$out"; GITHUB_OUTPUT=$out step pending "$NAME" > /dev/null 2> "$work/case/err"; }

echo "# the publish job's check of the artifacts"
fresh; step publish "$CHECK" > /dev/null; check $? 0 "the check accepts the four files"
fresh; rm assets/quadratique-0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256; step publish "$CHECK" > /dev/null 2>&1; check $? 1 "the check refuses a missing SHA-256 file"
fresh; echo extra > assets/notes.txt; step publish "$CHECK" > /dev/null 2>&1; check $? 1 "the check refuses an unexpected file"
fresh; echo tampered >> assets/quadratique-0.1.0-x86_64-unknown-linux-gnu.tar.gz; step publish "$CHECK" > /dev/null 2>&1; check $? 1 "the check refuses an archive that does not match its SHA-256 file"
fresh; cp assets/quadratique-0.1.0-aarch64-unknown-linux-gnu.tar.gz.sha256 assets/quadratique-0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256; step publish "$CHECK" > /dev/null 2>&1; check $? 1 "the check refuses a SHA-256 file that names the other archive"
fresh; STEP_VERSION=0.2.0 step publish "$CHECK" > /dev/null 2>&1; check $? 1 "the check refuses archives of another version"

echo "# the publish job's attach and publication"
fresh; step publish "$ATTACH" > /dev/null; check $? 0 "a draft with no files gets the four and is published"
check "$(draft) $(attached)" "False $four" "the published release holds the four files"
fresh; attach_all; edit "s['releases'][0]['draft'] = False"; step publish "$ATTACH" > /dev/null; check $? 0 "a public release with the four matching digests is complete"
check "$(changes)" 0 "and nothing is changed on it"
fresh; attach_all; edit "s['releases'][0]['draft'] = False"; echo wrong > gh/assets/101; step publish "$ATTACH" > /dev/null 2>&1; check $? 1 "a public release with a wrong file fails"
check "$(changes)" 0 "and nothing is changed on it"
fresh; attach_all; edit "s['releases'][0]['draft'] = False; s['releases'][0]['assets'].pop()"; step publish "$ATTACH" > /dev/null 2>&1; check $? 1 "a public release with a missing file fails"
check "$(changes)" 0 "and nothing is changed on it"
fresh; edit "s['fail_upload_of'] = 'quadratique-0.1.0-x86_64-unknown-linux-gnu.tar.gz'"; step publish "$ATTACH" > /dev/null 2>&1; check $? 1 "an attach whose upload fails fails"
check "$(draft)" True "the release is still a draft after a failed upload"
: > gh/calls; step publish "$ATTACH" > /dev/null; check $? 0 "running the attach again succeeds"
check "$(draft) $(attached)" "False $four" "the second run leaves the four files and publishes"
check "$(grep -c -- '-X POST' gh/calls)" 2 "the second run uploads only the file that was missing and its checksum"
fresh; attach_all; echo "stale copy" > gh/assets/104; edit "s['releases'][0]['assets'].append({'id': 99, 'name': 'quadratique-0.0.9-x86_64-unknown-linux-gnu.tar.gz'})"; echo old > gh/assets/99
step publish "$ATTACH" > /dev/null; check $? 0 "an attach over a stale copy and a foreign file succeeds"
check "$(draft) $(attached)" "False $four" "the stale copy is replaced and the foreign file removed"
fresh; edit "s['corrupt_upload_of'] = 'quadratique-0.1.0-aarch64-unknown-linux-gnu.tar.gz'"; step publish "$ATTACH" > /dev/null 2>&1; check $? 1 "an attach whose uploaded file differs from the checked one fails"
check "$(draft)" True "and leaves the release a draft"
fresh; edit "s['tags']['v0.1.0'] = '$OTHER'"; step publish "$ATTACH" > /dev/null 2>&1; check $? 1 "an attach fails when the tag no longer points at the built commit"
check "$(changes)" 0 "and changes nothing"
fresh; STEP_RELEASE=8 step publish "$ATTACH" > /dev/null 2>&1; check $? 1 "an attach to a release that does not exist fails"
check "$(changes)" 0 "and changes nothing"

echo "# naming the release to finish"
fresh; STEP_CREATED=true STEP_CREATED_TAG=v0.1.0 STEP_CREATED_SHA=$MERGE GITHUB_SHA=$MERGE name_release; check $? 0 "a draft that release-please just created is named"
check "$(tr '\n' ' ' < "$out")" "tag=v0.1.0 sha=$MERGE version=0.1.0 release=7 " "with its tag, commit, version and release ID"
fresh; STEP_CREATED='' GITHUB_SHA=$MERGE name_release; check $? 0 "after release-please failed once it had made the draft, the draft is named"
check "$(tr '\n' ' ' < "$out")" "tag=v0.1.0 sha=$MERGE version=0.1.0 release=7 " "and resumed at its tag's commit"
fresh; STEP_CREATED=false GITHUB_SHA=$LATER name_release; check $? 0 "a later push names the unfinished draft"
check "$(grep -c release=7 "$out")" 1 "the later push resumes release 7"
fresh; edit "s['releases'].insert(0, {'id': 5, 'tag_name': 'v0.1.0-rc.1', 'draft': True, 'target_commitish': '$MERGE', 'assets': []}); s['releases'].insert(0, {'id': 6, 'tag_name': 'v0.2.0', 'draft': True, 'target_commitish': '$MERGE', 'assets': []})"
STEP_CREATED=false GITHUB_SHA=$LATER name_release; check "$(grep release= "$out")" "release=7" "among drafts v0.2.0, v0.1.0-rc.1 and v0.1.0, listed first to last, the manifest's v0.1.0 is named"
fresh; edit "s['tags']['v0.1.0'] = '$OTHER'; s['releases'][0]['target_commitish'] = '$OTHER'"; STEP_CREATED=true STEP_CREATED_TAG=v0.1.0 STEP_CREATED_SHA=$MERGE GITHUB_SHA=$MERGE name_release; check $? 1 "a tag that points at another commit than release-please released fails"
check "$(wc -c < "$out")" 0 "and names nothing"
fresh; edit "s['tags']['v0.1.0'] = '$OTHER'; s['releases'][0]['target_commitish'] = '$OTHER'"; STEP_CREATED='' GITHUB_SHA=$LATER name_release; check $? 1 "on a later push, a tag on a commit whose manifest says another version fails"
fresh; edit "s['compare'] = {}"; STEP_CREATED='' GITHUB_SHA=$LATER name_release; check $? 1 "on a later push, a tag on a commit outside the pushed history fails"
fresh; edit "s['releases'][0]['target_commitish'] = '$OTHER'"; STEP_CREATED='' GITHUB_SHA=$LATER name_release; check $? 1 "a draft that targets another commit than its tag fails"
fresh; edit "s['releases'] = []"; STEP_CREATED=false GITHUB_SHA=$LATER name_release; check $? 0 "with no release for the manifest's version, naming succeeds"
check "$(wc -c < "$out")" 0 "and nothing is built"
fresh; attach_all; edit "s['releases'][0]['draft'] = False"; STEP_CREATED=false GITHUB_SHA=$LATER name_release; check $? 0 "a published release with its four files needs nothing"
check "$(wc -c < "$out")" 0 "and nothing is built"
fresh; attach_all; edit "s['releases'][0]['draft'] = False"; echo wrong > gh/assets/101; STEP_CREATED=false GITHUB_SHA=$LATER name_release; check $? 1 "a published release whose archive does not match its SHA-256 file fails"
check "$(changes)" 0 "and nothing is changed on it"
fresh; edit "s['releases'].append(dict(s['releases'][0], id=8))"; STEP_CREATED=false GITHUB_SHA=$LATER name_release; check $? 1 "two releases with the same tag fail"

echo "# the build's artifact"
check "$(uv run --quiet --with pyyaml==6.0.3 python -I -c "import sys, yaml; steps = yaml.safe_load(open(sys.argv[1]))['jobs']['binaries']['steps']; print(next(s for s in steps if s.get('uses', '').startswith('actions/upload-artifact'))['with'].get('overwrite'))" "$workflow")" True "a re-run of a build replaces its artifact (overwrite: true)"

[ "$failures" = 0 ] || { echo "$failures case(s) failed"; exit 1; }
