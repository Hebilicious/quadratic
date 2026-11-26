#!/bin/bash
# Runs the release workflow's own step scripts against a fake `gh` (./gh), a fake crates.io index (./curl), a fake
# `cargo` (./cargo) and a folder of artifacts, case by case.
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
CRATES="Name the crates that crates.io lacks"
VERSIONS="Check that both crates have the release's version"
PACKAGES="Package the crates that crates.io lacks and check them against the built ones"
PUBLISH="Publish the checked packages"
TAGCHECK="Check that the tag points at this commit"
RECORD="Record the SHA-256 of each package"
MERGE=1111111111111111111111111111111111111111
LATER=2222222222222222222222222222222222222222
OTHER=3333333333333333333333333333333333333333
failures=0
check() {
  if [ "$1" = "$2" ]; then echo "ok: $3"; else echo "FAIL: $3 (got $1, wanted $2)"; failures=$((failures + 1)); fi
}

# A repository with the merged release commit $MERGE (manifest 0.1.0), a later commit $LATER, and a draft release
# v0.1.0 (id 7) whose tag points at $MERGE; the four built files are in $work/assets. Both crates are at 0.1.0 in
# the workspace, and crates.io has only 0.0.9 of each.
fresh() {
  rm -rf "$work/case" && mkdir -p "$work/case/assets" "$work/case/gh/assets" "$work/case/apps/quadratique"
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
 "compare": {"$MERGE...$LATER": "ahead", "$MERGE...$MERGE": "identical"},
 "workspace": {"quadratique-server": "0.1.0", "quadratique": "0.1.0", "requires": "^0.1.0"},
 "crates": {"quadratique-server": ["0.0.9"], "quadratique": ["0.0.9"]}}
JSON
  export FAKE_GH=$work/case/gh STEP_VERSION=0.1.0 STEP_TAG=v0.1.0 STEP_RELEASE=7 STEP_SHA=$MERGE STEP_CARGO_REGISTRY_TOKEN=fake
  export STEP_CHECKSUMS="quadratique-server=$(built quadratique-server) quadratique=$(built quadratique)"
  cd "$work/case" || exit 1
}
edit() { python3 -c "import json,sys; p='$work/case/gh/state.json'; s=json.load(open(p)); exec(sys.argv[1]); json.dump(s, open(p,'w'))" "$1"; }
draft() { python3 -c "import json; print(json.load(open('$work/case/gh/state.json'))['releases'][0]['draft'])"; }
attached() { python3 -c "import json; print(' '.join(sorted(a['name'] for a in json.load(open('$work/case/gh/state.json'))['releases'][0]['assets'])))"; }
changes() { grep -cE -- '-X (DELETE|POST|PATCH)' "$work/case/gh/calls" 2> /dev/null || true; }
attach_all() { (cd assets && for f in *; do gh api -X POST "https://uploads.github.com/repos/Hebilicious/quadratique/releases/7/assets?name=$f" -H "Content-Type: application/octet-stream" --input "$f" > /dev/null; done); : > gh/calls; }
four="quadratique-0.1.0-aarch64-unknown-linux-gnu.tar.gz quadratique-0.1.0-aarch64-unknown-linux-gnu.tar.gz.sha256 quadratique-0.1.0-x86_64-unknown-linux-gnu.tar.gz quadratique-0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256"
name_release() { out=$work/case/output; : > "$out"; GITHUB_OUTPUT=$out step pending "$NAME" > /dev/null 2> "$work/case/err"; }
name_crates() { out=$work/case/output; : > "$out"; GITHUB_OUTPUT=$out step crates "$CRATES" > /dev/null 2> "$work/case/err"; }
# The SHA-256 of the package that the fake cargo makes of a crate at 0.1.0.
built() { printf '%s 0.1.0' "$1" | sha256sum | cut -d ' ' -f 1; }
# crates.io lists the crate at 0.1.0, with the SHA-256 of the built package unless another is given.
on_crates_io() { edit "s['crates']['$1'].append('0.1.0'); s.setdefault('cksums', {})['$1@0.1.0'] = '${2:-$(built "$1")}'"; }
# The publish job's two steps, as GitHub runs them: the second runs only when the first names crates.
publish_crates() {
  local out=$work/case/output crates
  : > "$out"
  GITHUB_OUTPUT=$out step crates-publish "$PACKAGES" > /dev/null 2> "$work/case/err" || return 1
  crates=$(sed -n 's/^crates=//p' "$out")
  [ -n "$crates" ] || return 0
  STEP_CRATES=$crates step crates-publish "$PUBLISH" > /dev/null 2>> "$work/case/err"
}
listed() { python3 -c "import json; c=json.load(open('$work/case/gh/state.json'))['crates']; print(' '.join(n for n in ('quadratique-server', 'quadratique') if '0.1.0' in c.get(n, [])) or 'none')"; }
publications() { grep '^cargo publish' "$work/case/gh/calls" 2> /dev/null | sed 's/^cargo publish --locked --no-verify --allow-dirty //' | paste -sd ';' -; }
packagings() { grep '^cargo package --no-verify' "$work/case/gh/calls" 2> /dev/null | sed 's/^cargo package --no-verify --locked --allow-dirty //' | paste -sd ';' -; }

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
check "$(tr '\n' ' ' < "$out")" "tag=v0.1.0 sha=$MERGE version=0.1.0 release=7 crates-sha=$MERGE " "with its tag, commit, version and release ID, and the commit of its crates"
fresh; STEP_CREATED='' GITHUB_SHA=$MERGE name_release; check $? 0 "after release-please failed once it had made the draft, the draft is named"
check "$(tr '\n' ' ' < "$out")" "tag=v0.1.0 sha=$MERGE version=0.1.0 release=7 crates-sha=$MERGE " "and resumed at its tag's commit"
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
fresh; attach_all; edit "s['releases'][0]['draft'] = False"; STEP_CREATED=false GITHUB_SHA=$LATER name_release; check $? 0 "a published release with its four files needs no build"
check "$(tr '\n' ' ' < "$out")" "version=0.1.0 crates-sha=$MERGE " "and only its version and the commit of its crates are named"
fresh; attach_all; edit "s['releases'][0]['draft'] = False; s['compare'] = {}"; STEP_CREATED=false GITHUB_SHA=$LATER name_release; check $? 1 "a published release whose tag is outside the pushed history fails"
check "$(wc -c < "$out")" 0 "and names nothing"
fresh; attach_all; edit "s['releases'][0]['draft'] = False; s['tags']['v0.1.0'] = '$OTHER'"; STEP_CREATED=false GITHUB_SHA=$LATER name_release; check $? 1 "a published release whose tag is on a commit of another version fails"
fresh; attach_all; edit "s['releases'][0]['draft'] = False; s['tags']['v0.1.0'] = '$LATER'; s['compare']['$LATER...$LATER'] = 'identical'"; STEP_CREATED=false GITHUB_SHA=$LATER name_release; check $? 1 "a published release whose tag moved to a later commit of the same version fails (F2)"
check "$(wc -c < "$out")" 0 "and names no commit for its crates"
grep -q "was built from $MERGE" err; check $? 0 "and says which commit the release was built from"
fresh; attach_all; edit "s['releases'][0]['draft'] = False"; echo wrong > gh/assets/101; STEP_CREATED=false GITHUB_SHA=$LATER name_release; check $? 1 "a published release whose archive does not match its SHA-256 file fails"
check "$(changes)" 0 "and nothing is changed on it"
fresh; edit "s['releases'].append(dict(s['releases'][0], id=8))"; STEP_CREATED=false GITHUB_SHA=$LATER name_release; check $? 1 "two releases with the same tag fail"

echo "# the release's publication does not wait for crates.io"
check "$(uv run --quiet --with pyyaml==6.0.3 python -I -c "
import sys, yaml
jobs = yaml.safe_load(open(sys.argv[1]))['jobs']
def needs(name):
    n = jobs[name].get('needs', [])
    n = [n] if isinstance(n, str) else n
    return set(n).union(*[needs(m) for m in n]) if n else set()
print(sorted(j for j in needs('publish') | {'publish'} if 'crates.io' in str(jobs[j])))" "$workflow")" "[]" "no job that the release's publication waits for reads crates.io (F1)"
check "$(uv run --quiet --with pyyaml==6.0.3 python -I -c "import sys, yaml; print(sorted(yaml.safe_load(open(sys.argv[1]))['jobs']['crates']['needs']))" "$workflow")" "['pending', 'publish']" "crates.io's index is read once the release is published"

echo "# naming the crates that crates.io lacks"
fresh; name_crates; check $? 0 "with neither crate at 0.1.0 on crates.io, naming succeeds"
check "$(cat "$out")" "crates=quadratique-server quadratique" "and names both, the server first"
fresh; on_crates_io quadratique-server; name_crates; check "$(cat "$out")" "crates=quadratique" "with the server already published, only the command line is named"
fresh; on_crates_io quadratique-server; on_crates_io quadratique; name_crates; check $? 0 "with both crates published, naming succeeds"
check "$(wc -c < "$out")" 0 "and names nothing, so no crates job runs"
fresh; edit "s['crates'] = {}"; name_crates; check "$(cat "$out")" "crates=quadratique-server quadratique" "a crate that the index does not know (404) is named"
fresh; edit "s['index_status'] = 500"; name_crates; check $? 1 "an index that answers 500 fails"
check "$(wc -c < "$out")" 0 "and names nothing"
fresh; edit "s['index_body'] = '<html>not json</html>'"; name_crates; check $? 1 "an index answer that is not JSON fails (F4)"
check "$(wc -c < "$out")" 0 "and names nothing"
fresh; edit "s['index_body'] = '{\"name\": \"quadratique\"}'"; name_crates; check $? 1 "an index line without a version and a checksum fails (F4)"
fresh; edit "s['index_body'] = '{\"vers\": \"0.0.9\", \"cksum\": \"00\"}\n[\"not an object\"]'"; name_crates; check $? 1 "an index line that is not an object fails (F4)"

echo "# the build job"
fresh; step crates-build "$VERSIONS" > /dev/null; check $? 0 "the check accepts both crates at the release's version"
fresh; edit "s['workspace']['quadratique-server'] = '0.2.0'"; step crates-build "$VERSIONS" > /dev/null 2>&1; check $? 1 "the check refuses a server crate at another version than the tag"
fresh; edit "s['workspace']['quadratique'] = '0.0.9'"; step crates-build "$VERSIONS" > /dev/null 2>&1; check $? 1 "the check refuses a command line crate at another version than the tag"
fresh; edit "s['workspace']['requires'] = '^0.0.9'"; step crates-build "$VERSIONS" > /dev/null 2>&1; check $? 1 "the check refuses a command line that requires another server version"
fresh; STEP_VERSION=0.2.0 step crates-build "$VERSIONS" > /dev/null 2>&1; check $? 1 "the check refuses crates whose version is not the tag's"
fresh; (cd apps/quadratique && cargo package --workspace --allow-dirty --locked); out=$work/case/output; : > "$out"; GITHUB_OUTPUT=$out step crates-build "$RECORD" > /dev/null
check "$(cat "$out")" "checksums=$STEP_CHECKSUMS" "the build records the SHA-256 of both packages"
for job in crates-build crates-publish; do
  fresh; git init -q . && git -c user.name=t -c user.email=t@example.com commit -q --allow-empty -m tagged && git tag v0.1.0
  step $job "$TAGCHECK" > /dev/null 2>&1; check $? 0 "$job: the tag check accepts the tag's commit"
  git -c user.name=t -c user.email=t@example.com commit -q --allow-empty -m later; step $job "$TAGCHECK" > /dev/null 2>&1; check $? 1 "$job: the tag check refuses another commit"
done

echo "# the publish job"
fresh; publish_crates; check $? 0 "with both crates new, the publication succeeds"
check "$(listed)" "quadratique-server quadratique" "and crates.io lists both"
check "$(packagings)" "--package=quadratique-server --package=quadratique" "after it packaged both without building them"
check "$(publications)" "--package=quadratique-server --package=quadratique" "in one cargo publish, which uploads the server before the command line"
fresh; on_crates_io quadratique-server; publish_crates; check $? 0 "with the server already published, the publication succeeds"
check "$(publications)" "--package=quadratique" "and publishes only the command line"
fresh; on_crates_io quadratique-server; on_crates_io quadratique; publish_crates; check $? 0 "with both already published, the publication succeeds"
check "$(packagings) $(publications)" " " "and packages and publishes nothing"
fresh; edit "s['fail_publish_of'] = 'quadratique'"; publish_crates; check $? 1 "when the command line's upload fails, the publication fails"
check "$(listed)" "quadratique-server" "with only the server published"
: > gh/calls; publish_crates; check $? 0 "running the failed job again succeeds"
check "$(publications)" "--package=quadratique" "and publishes only the command line"
check "$(listed)" "quadratique-server quadratique" "after which crates.io lists both"
fresh; edit "s['unlisted'] = 'quadratique-server'"; publish_crates; check $? 1 "when the index does not list the server within cargo's wait, the publication fails"
check "$(listed)" "none" "before the command line is uploaded"
on_crates_io quadratique-server; edit "del s['unlisted']"; : > gh/calls; publish_crates; check $? 0 "once the index lists the server, running the job again succeeds"
check "$(publications)" "--package=quadratique" "and publishes only the command line"
fresh; on_crates_io quadratique-server; edit "s['unlisted'] = 'quadratique'"; publish_crates; check $? 1 "an upload that the index does not list when cargo returns fails the job"
grep -q "does not list quadratique 0.1.0 yet" err; check $? 0 "and says to check crates.io and run the failed jobs again"
fresh; edit "s['index_status'] = 500"; publish_crates; check $? 1 "an index that answers 500 fails the publication"
check "$(packagings) $(publications)" " " "before anything is packaged or published"
fresh; edit "s['index_body'] = '<html>not json</html>'"; publish_crates; check $? 1 "an index answer that is not JSON fails the publication (F4)"
check "$(publications)" "" "and runs no cargo publish"
fresh; edit "s['package_differs'] = {'quadratique': ' with another lock file'}"; publish_crates; check $? 1 "a package that differs from the built one stops the job (F3)"
check "$(publications)" "" "before the token and the upload"
grep -q "not the $(built quadratique) that the job 'build the crates' built" err; check $? 0 "and names both SHA-256"
fresh; on_crates_io quadratique-server 1111111111111111111111111111111111111111111111111111111111111111; publish_crates; check $? 1 "a server on crates.io with another SHA-256 than the built one stops the job"
check "$(publications)" "" "and nothing is published"
fresh; edit "s['publish_differs'] = {'quadratique': ' changed'}"; publish_crates; check $? 1 "an upload whose SHA-256 on crates.io is not the built one fails the job"
fresh; STEP_CHECKSUMS="quadratique=$(built quadratique)"; publish_crates; check $? 1 "a build that recorded no SHA-256 for a crate stops the job"
check "$(packagings) $(publications)" " " "before anything is packaged or published"

echo "# what each crates job holds"
job() { uv run --quiet --with pyyaml==6.0.3 python -I -c "import sys, yaml; jobs = yaml.safe_load(open(sys.argv[1]))['jobs']; job = jobs[sys.argv[2]]; text = str(job['steps']); print(eval(sys.argv[3]))" "$workflow" "$1" "$2"; }
check "$(job crates "sorted(n for n, j in jobs.items() if 'environment' in j or 'id-token' in j.get('permissions', {}))")" "['crates-publish']" "only the publish job runs in an environment or may ask for an OpenID Connect token"
check "$(job crates-publish "sorted(job['permissions'].items())")" "[('contents', 'read'), ('id-token', 'write')]" "the publish job may only read the repository and ask for an OpenID Connect token"
check "$(job crates "sorted(n for n in ('crates', 'crates-build', 'crates-publish') if 'timeout-minutes' not in jobs[n])")" "[]" "each crates job has a time limit, so a run that hangs does not hold the next push to main"
check "$(grep -c 'curl ' "$workflow") $(grep -c 'curl .*--connect-timeout 10 --max-time 60 ' "$workflow")" "3 3" "every read of crates.io's index gives up after a minute"
check "$(uv run --quiet --with pyyaml==6.0.3 python -I "$here/moon_targets.py" "$here/../..")" "" "every moon task that a workflow runs is one that moon runs in CI"
check "$(job crates-publish "job.get('environment')")" "crates-io" "it runs in the environment crates-io, which crates.io's configurations name"
check "$(job crates-publish "[w for w in ('pnpm', 'moon', 'setup-toolchain', 'cargo build', 'cargo test', 'cargo package --workspace', 'make') if w in text] + [l.strip() for l in text.split(chr(92) + 'n') if l.strip().startswith(('cargo package', 'cargo publish')) and '--no-verify' not in l]")" "[]" "it installs nothing from the project and builds nothing (F3)"
check "$(job crates-publish "[s.get('id') or s.get('name') for s in job['steps']][-3:]")" "['packages', 'crates-io', 'Publish the checked packages']" "it gets the crates.io token after the packages are checked, right before publishing"
check "$(job crates-publish "sorted(job['needs'])")" "['crates-build', 'pending']" "it waits for the build job"
check "$(job crates-build "(job.get('permissions'), job.get('environment'), sorted(job['needs']))")" "(None, None, ['crates', 'pending'])" "the build job holds only the workflow's read access and runs in no environment"
for name in crates crates-build crates-publish; do
  check "$(job $name "'secrets.' in str(job) or 'create-github-app-token' in text")" False "$name reads no secret and mints no App token"
  check "$(job $name "[s['with'].get('persist-credentials') for s in job['steps'] if s.get('uses', '').startswith('actions/checkout')] in ([], [False])")" True "$name's checkout keeps no credentials"
done

echo "# the build's artifact"
check "$(uv run --quiet --with pyyaml==6.0.3 python -I -c "import sys, yaml; steps = yaml.safe_load(open(sys.argv[1]))['jobs']['binaries']['steps']; print(next(s for s in steps if s.get('uses', '').startswith('actions/upload-artifact'))['with'].get('overwrite'))" "$workflow")" True "a re-run of a build replaces its artifact (overwrite: true)"

[ "$failures" = 0 ] || { echo "$failures case(s) failed"; exit 1; }
