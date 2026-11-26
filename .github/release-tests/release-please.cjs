// Runs release-please, as bundled in the release-please-action commit that .github/workflows/release.yml pins, on
// this repository's configuration, with no network calls to the GitHub API and no token. It checks both halves of a
// release: the release pull request it opens, and the release (tag, draft, forced tag) that the merged pull request
// yields. It downloads the action's bundle from raw.githubusercontent.com.
//
// Run from the repository root: node .github/release-tests/release-please.cjs
"use strict";
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const assert = require("node:assert/strict");

const repository = path.resolve(__dirname, "../..");
const workflow = fs.readFileSync(path.join(repository, ".github/workflows/release.yml"), "utf8");
const pin = /googleapis\/release-please-action@([0-9a-f]{40})/.exec(workflow)[1];
const TEMPLATES = ["commit.hbs", "commit1.hbs", "commit2.hbs", "footer.hbs", "footer1.hbs", "header.hbs", "header1.hbs",
  "header2.hbs", "template.hbs", "template1.hbs", "template2.hbs"];

async function download(folder) {
  for (const file of ["index.js", ...TEMPLATES]) {
    const url = `https://raw.githubusercontent.com/googleapis/release-please-action/${pin}/dist/${file}`;
    const response = await fetch(url);
    if (!response.ok) throw new Error(`${url}: ${response.status}`);
    fs.writeFileSync(path.join(folder, file), Buffer.from(await response.arrayBuffer()));
  }
}

// The bundle exposes only the action. Patch it to also hand out its module loader, then load the modules that
// export release-please's GitHub, Manifest, logger and errors.
function load(folder) {
  const text = fs.readFileSync(path.join(folder, "index.js"), "utf8");
  const patched = text.replace(/module\.exports = __webpack_exports__;\s*\/\*{6}\/ \}\)\(\)\s*;?\s*$/,
    "module.exports = { require: __nccwpck_require__ };\n})();\n");
  assert.notEqual(patched, text, "the bundle does not end as expected");
  const copy = path.join(folder, "patched.cjs");
  fs.writeFileSync(copy, patched);
  const load = require(copy).require;
  const moduleOf = (marker) => {
    const at = text.indexOf(marker);
    assert.ok(at >= 0, `the bundle has no ${marker}`);
    const id = /\/\*{3}\/ (\d+):/g;
    let found;
    for (let match; (match = id.exec(text)) && match.index < at;) found = match[1];
    return load(Number(found));
  };
  const version = /exports\.VERSION = '([0-9.]+)'/.exec(text)[1];
  return {
    version,
    readsAlwaysUpdate: /\['always-update'\]|"always-update"/.test(text),
    GitHub: moduleOf("exports.GitHub = GitHub").GitHub,
    Manifest: moduleOf("exports.Manifest = Manifest").Manifest,
    setLogger: moduleOf("exports.setLogger = setLogger").setLogger,
    FileNotFoundError: moduleOf("exports.FileNotFoundError = FileNotFoundError").FileNotFoundError,
  };
}

// A GitHub client that reads files from this checkout, knows no release or tag, and lists the given commits since
// the bootstrap commit and, once given, the merged release pull request.
function client(lib, merged) {
  const config = JSON.parse(fs.readFileSync(path.join(repository, "release-please-config.json"), "utf8"));
  const github = Object.create(lib.GitHub.prototype);
  github.repository = { owner: "Hebilicious", repo: "quadratique", defaultBranch: "main" };
  github.logger = silent;
  github.getFileContentsOnBranch = async (file) => {
    const full = path.join(repository, file);
    if (!fs.existsSync(full)) throw new lib.FileNotFoundError(file);
    const text = fs.readFileSync(full, "utf8");
    return { sha: "blob", content: Buffer.from(text).toString("base64"), parsedContent: text, mode: "100644" };
  };
  github.getFileContents = (file) => github.getFileContentsOnBranch(file, "main");
  github.getFileJson = async (file) => JSON.parse(fs.readFileSync(path.join(repository, file), "utf8"));
  github.releaseIterator = async function* () {};
  github.tagIterator = async function* () {};
  github.mergeCommitIterator = async function* () {
    yield { sha: "1111111111111111111111111111111111111111", message: "feat: a change that releases", files: ["README.md"] };
    yield { sha: config["bootstrap-sha"], message: "the bootstrap commit", files: ["README.md"] };
  };
  github.pullRequestIterator = async function* (_branch, status) {
    if (merged && status === "MERGED") yield merged;
  };
  for (const name of ["findFilesByFilenameAndRef", "findFilesByGlobAndRef", "findFilesByExtensionAndRef",
    "findFilesByExtension", "findFilesByFilename", "findFilesByGlob"]) {
    github[name] = async () => [];
  }
  return github;
}

const silent = { error() {}, warn() {}, info() {}, debug() {}, trace() {} };

(async () => {
  const folder = fs.mkdtempSync(path.join(os.tmpdir(), "release-please-action-"));
  try {
    await download(folder);
    const lib = load(folder);
    lib.setLogger(silent);
    const config = JSON.parse(fs.readFileSync(path.join(repository, "release-please-config.json"), "utf8"));

    // Without always-update, release-please leaves the release pull request on an older main when a push does not
    // change the changelog; a push that changed a file the pull request edits (Cargo.lock) then leaves it in conflict.
    assert.equal(config["always-update"], true, "the configuration asks for a release pull request that follows main");
    assert.ok(lib.readsAlwaysUpdate, "the pinned release-please reads always-update");
    console.log("ok: the release pull request is written again at every push to main (always-update)");

    const opened = await (await lib.Manifest.fromManifest(client(lib), "main")).buildPullRequests();
    assert.equal(opened.length, 1, "release-please opens one release pull request");
    const pullRequest = opened[0];
    const title = pullRequest.title.toString();
    const version = /^chore\(release\): release (\S+)$/.exec(title)?.[1];
    assert.ok(version, `the release pull request's title names the version: ${title}`);
    if (config.packages["."]["release-as"]) assert.equal(version, config.packages["."]["release-as"]);
    console.log(`ok: release-please ${lib.version} (action ${pin.slice(0, 7)}) opens "${title}" on ${pullRequest.headRefName}`);

    const merged = {
      headBranchName: pullRequest.headRefName, baseBranchName: "main", number: 99, title, body: pullRequest.body.toString(),
      labels: ["autorelease: pending"], files: [], sha: "2222222222222222222222222222222222222222",
    };
    const releases = await (await lib.Manifest.fromManifest(client(lib, merged), "main")).buildReleases();
    assert.equal(releases.length, 1, "the merged release pull request yields one release");
    const [release] = releases;
    assert.equal(release.tag.toString(), `v${version}`);
    assert.equal(release.sha, merged.sha);
    assert.equal(release.draft, true, "the release is a draft");
    assert.equal(release.forceTag, true, "the tag is created with the draft");
    console.log(`ok: the merged pull request yields the draft release ${release.tag} at its merge commit, with a forced tag`);
  } finally {
    fs.rmSync(folder, { recursive: true, force: true });
  }
})().catch((error) => {
  console.error(`FAIL: ${error.message}`);
  process.exit(1);
});
