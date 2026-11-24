# Quadratic

Quadratic is a local issue tracker for one human and many AI agents. Agents learn to use it
from the skill that `quadratic init` writes into a repository and from `quadratic instructions`.
This file is for whoever installs, runs or changes Quadratic.

One server owns the store. The `quadratic` command line, the MCP server (`quadratic mcp`) and
the web app all talk to it. A command starts the server when none answers.

## Before you start

The install and every task run through [moon](https://moonrepo.dev/moon), with the tools that
`.prototools` pins: moon, Node.js, pnpm, Rust, Pkl and the Java runtime it needs, Python and uv.
[proto](https://moonrepo.dev/proto) installs them. The Rust build also needs a C compiler and the
zlib development files, because the store engine is compiled from C (on Debian or Ubuntu:
`sudo apt install build-essential zlib1g-dev`). Once, from the root of the repository:

```sh
proto use      # installs the pinned tools
pnpm install   # installs the JavaScript dependencies, including the acid-cat command
```

The browser tests (`quadratic-web:e2e`) also need the Chromium that Playwright drives:

```sh
pnpm --filter quadratic-web exec playwright install chromium-headless-shell
```

The tests also call `jq`, `curl`, `flock`, `systemctl --user` and `unshare --user`, so they need
those commands and unprivileged user namespaces.

On a minimal Linux host, that Chromium also needs system libraries. On Debian or Ubuntu,
Playwright installs them (it asks for `sudo`):

```sh
pnpm --filter quadratic-web exec playwright install-deps chromium-headless-shell
```

## Install

On Linux with systemd, from the root of the repository:

```sh
moon run quadratic-install:install
```

This does four things:

1. Builds the release binary with the web app inside it.
2. Installs the binary as `$XDG_BIN_HOME/quadratic`, or `~/.local/bin/quadratic` when
   `XDG_BIN_HOME` is not set.
3. Writes the systemd user unit `~/.config/systemd/user/quadratic.service`, then enables and
   restarts it. The server then starts with your session. When it crashes or fails to start,
   systemd starts it again after 2 seconds, as often as needed: the unit has no start limit.
   A server that exits with status 5, as it does when its store cannot open, is not restarted:
   `systemctl --user status quadratic.service` shows why it stopped.
   A stop may take up to 70 seconds while the server closes its store; then systemd ends the
   process (`TimeoutStopSec=70`). Every write the server answered is kept.
4. Waits until the unit's main process is the new binary's server and answers. If it is not
   within 60 seconds, or the restart fails, the install fails and prints
   `systemctl --user status quadratic.service`.

Once it is done, the checkout is not needed: you can delete it.

The install refuses to run when `XDG_CONFIG_HOME` is set to anything but `~/.config`. The
systemd user manager looks for units there only when its own environment sets that
variable, so a unit written elsewhere might never start. Run the install with
`XDG_CONFIG_HOME` unset. The install also refuses a binary or data folder whose path has
`"`, `\`, `%`, `$` or a control character, which a unit would read differently.

Check that the server runs, and open the web app:

```sh
quadratic server status
quadratic ui --open
```

The port changes each time the server starts. `quadratic ui` prints the current address.

## Install from a release

Each [GitHub release](https://github.com/Hebilicious/quadratic/releases) has a prebuilt binary,
with the web app inside it, for Linux on x86-64 and on arm64. It needs glibc 2.34 or newer
(Ubuntu 22.04, Debian 12, RHEL 9, Fedora 35 or later) and no other tool: no Rust, Node.js or
proto.

Each platform has one archive, named for the version and the target, and its SHA-256 file:

| Platform | Archive |
|---|---|
| Linux x86-64 | `quadratic-<version>-x86_64-unknown-linux-gnu.tar.gz` |
| Linux arm64 | `quadratic-<version>-aarch64-unknown-linux-gnu.tar.gz` |

The archive has one folder, `quadratic-<version>-<target>/`, with the binary `quadratic`, the
scripts `install.sh` and `uninstall.sh`, this file and the license. `install.sh` is the script
that `moon run quadratic-install:install` runs, and does the steps 2 to 4 of "Install" with the
binary you give it. Download, check and install a release (here 0.2.0 for x86-64; use
`aarch64` for arm64):

```sh
version=0.2.0
archive=quadratic-$version-x86_64-unknown-linux-gnu
curl -fLO "https://github.com/Hebilicious/quadratic/releases/download/v$version/$archive.tar.gz"
curl -fLO "https://github.com/Hebilicious/quadratic/releases/download/v$version/$archive.tar.gz.sha256"
sha256sum -c "$archive.tar.gz.sha256"
tar -xzf "$archive.tar.gz"
sh "$archive/install.sh" "$archive/quadratic"
```

To upgrade, do the same with the new release. To uninstall, run `sh "$archive/uninstall.sh"`
from any release: it removes the binary and the unit that the unit names.

### With mise

[mise](https://mise.jdx.dev) installs the binary from the GitHub releases. It picks the archive
for your platform from its name, checks it against the SHA-256 file, and puts `quadratic` on your
`PATH`:

```sh
mise use -g github:Hebilicious/quadratic
```

This installs only the binary. To run the server as a systemd user service, also run the
`install.sh` of that release with the binary that mise installed:

```sh
sh "$(mise where github:Hebilicious/quadratic)/install.sh" "$(mise which quadratic)"
```

The install copies the binary to `~/.local/bin/quadratic`, so the service keeps the version you
installed until you run `install.sh` again after `mise upgrade`. By default mise does not install a
release published less than 24 hours ago (its setting `minimum_release_age`); name the version to
install it earlier (`mise use -g github:Hebilicious/quadratic@0.2.0`).

## Set up a repository

From inside a git repository:

```sh
quadratic init --key ABC --name "My project"
```

`init` writes the skill to `.agents/skills/quadratic/` (`SKILL.md` and `planning.md`), adds a
line to `AGENTS.md` that points agents to the skill, and creates the project bound to the
repository. Without `--key`, the key comes from the folder's name. The project's default branch
comes from `--default-branch`, else from the remote's `HEAD`, else from the branch checked out in
the main worktree. When none of these names a branch, as in a main worktree with a detached
`HEAD` and no `origin/HEAD`, the project gets `main`, and the answer's `defaultBranchFrom` says
"nothing in the repository names one: give --default-branch". Change it later with
`quadratic project update`.

A skill file you changed is left as it is. Of the setup files, `init` writes only the three
above, all inside the repository, and writes no agent configuration. Like any command, it starts
the server when none answers, and the server keeps its store in the data folder (see "Backups"),
outside the repository. `init` makes no commit. Its answer has an optional `commit` field: the
`git add` and `git commit` to run for the setup files that `init` wrote, or that hold exactly its
own text and are not committed yet. A skill file you changed, which `init` left alone, is not in
that command. When every setup file is already committed, the answer has no `commit` field.

## Who can talk to the server

The server listens on `127.0.0.1`, and on Linux it answers only processes of the account that
runs it. At each connection it asks the kernel which account owns the caller's socket
(`NETLINK_SOCK_DIAG`). A process of any other account, `root` included, gets `403` "This server
answers only processes of its own account." A server that cannot ask the kernel who is
connecting, in a sandbox that forbids netlink or on a kernel without its `inet_diag` module, does
not start and says why. So does a server that runs in a user namespace where its account is not
mapped, where the kernel would show every unmapped caller as that same account. If a lookup
fails once the server runs, the server refuses that connection and says so once in its log.
Every process of your own account is you, whatever name it gives. The rule sees only the process at the other end of the connection: a proxy or port
forward that runs as you (`ssh -L`, socat, the userland forwarders of rootless containers) is
that process, so everyone who connects through it counts as you. No web page can call the routes
of the command line, not even Quadratic's own.

The server makes its data folder readable only by its own account (mode `0700`).

Some agent sandboxes forbid connections to the loopback interface, and some make the file system
read-only, so that a command can neither reach the server nor start or stop one in the data
folder. A command run in such a sandbox, or by another account, fails, and starts, creates and
stops nothing:

```text
Cannot reach the Quadratic server from here: <reason>. Nothing was started. Call the Quadratic MCP tool `<tool>` with the same inputs instead. The MCP tools act from the folder this session started in; for work in another worktree, pass its absolute path as `worktree` to `issue.claim`; once that claim succeeds, later calls act from it. If this session has no Quadratic MCP tools, tell the human that its harness must start `quadratic mcp` as an MCP server.
```

The reason is the one the command met, for example "connecting to 127.0.0.1:40091 is not
permitted" or "this process cannot write /home/you/.local/share/quadratic: Read-only file
system; a sandbox may be blocking it". This is the answer of a command whose capability is also
an MCP tool, and `<tool>` names that tool. Every other command that needs the server ends with
"Nothing was started. This command works only on the command line: run it outside the sandbox,
or ask the human to." Input the command line cannot match to a command (a typo after a group
word, a bare group, a flag other than `--json` before the command) ends with "Nothing was started. `quadratic --help`
lists the commands." `quadratic ui` and `quadratic serve --take-over`, which people run, end with
"Nothing was started." and no advice. `quadratic server stop` ends with "Nothing was stopped." and no advice. `quadratic server
status` answers `"reachable": false` and the reason. `quadratic mcp` checks at its start and
exits on standard error with "Nothing was started.", before it offers any tool; if the server
becomes unreachable later, it writes each reason once to standard error. "Agents in a sandbox"
below says how to let such an agent work.

## Agents in a sandbox

A Codex session that runs its commands in Codex's sandbox cannot use the `quadratic` command:
the sandbox blocks connections to `127.0.0.1`, so every command that needs the server answers
"Cannot reach the Quadratic server from here"; `--help`, `--version` and `server status` still
answer. Codex starts its MCP servers outside the sandbox, so the same session can use Quadratic
through `quadratic mcp`. The command's refusal names the MCP tool to call instead, and tells the
agent to ask you for the tools when the session has none. A command that has no MCP tool says
that it works only on the command line, to run outside the sandbox.

Give Codex the tools once, for every session of your account:

```sh
codex mcp add quadratic --env AI_AGENT=codex -- quadratic mcp
```

This writes `[mcp_servers.quadratic]` with `command`, `args` and `env` to `~/.codex/config.toml`.
Codex does not pass its own environment to an MCP server, so Quadratic cannot tell that the
caller is Codex: without `AI_AGENT=codex` the session's changes are by `agent@<folder>`, with
it by `codex@<folder>`.

The MCP server takes the project, the branch and the worktree from the folder where Codex
starts the session. An agent that works in another worktree names it in the claim
(`issue.claim` with `worktree`, the worktree's absolute path). The MCP server then sends the
claim from that worktree. Once the claim is taken, the session acts from that worktree until
another claim that names a worktree is taken. A claim that fails leaves the session where it
was. The server refuses a `worktree` that is not a worktree of the project's repository.

If the answer to such a claim is lost (the tool call ends in an error), the claim may still be
recorded while the session acts from the old folder. Read the issue (`issue.get`) to see whether
the claim is recorded, then repeat the same claim: the server changes nothing, and the session
moves to the worktree.

The other way is to let the sandbox reach the network. In `~/.codex/config.toml`:

```toml
[sandbox_workspace_write]
network_access = true
```

The `quadratic` command then works in the sandbox. The cost: every sandboxed command of every
Codex session can reach the network, not only Quadratic's server.

These facts were measured with codex-cli 0.171.0 and its default sandbox policy:
the blocked loopback, the MCP tools working in a sandboxed session while the command was
blocked, the project taken from the session's folder, the actor names with and without
`AI_AGENT`, what `codex mcp add` writes, and the command working with `network_access = true`.
A `.codex/config.toml` inside a project folder was not listed by `codex mcp list`, so this
section names only the file in your home.

## Agent tokens

An agent token names one agent, by a label, for an agent that will work from another machine
than the one that runs the server. No route accepts a token yet.
Only the command line manages tokens: no web page and no MCP tool can.

```sh
quadratic token create --label deploy --file /home/you/secrets/deploy.token
quadratic token list
quadratic token revoke --label deploy
```

`create` writes the token's value, `qdt_` followed by 43 characters, to the file that `--file`
names. The path must be absolute, and nothing may be at it yet: the server creates the file with
mode `0600` and refuses a path where a file or a link already is (`FileExists`), and a relative
path (`NotAbsolute`). The file cannot be in the data folder itself (`InDataFolder`), where the
server's own files would overwrite or remove it; a link to that folder counts as the folder. The value is in that file and nowhere else: not in the command's output,
not in the server's log, not in the store. The label is the agent's name. It follows the rule of a
role: lowercase letters, digits and single hyphens, at most 32 characters. Two tokens cannot share
a label, and no token can take the name of one of the server's own actors, `quadratic`, `agent`,
`browser` or `human` (`LabelTaken`).

`list` gives each token's label and the moment it was created. `revoke` forgets the token at
once, and answers `"revoked": false` when no token had the label. It does not delete the file.

The server keeps only the SHA-256 of each value, with its label and date, in `credentials.json`
beside `workspace.db` (mode `0600`). It is not in the store, so restoring a backup of
`workspace.db` brings back no revoked token. Restoring a copy of `credentials.json` brings back
the tokens it lists, including ones you revoked after the copy was made: after you restore one,
revoke every token it lists and create new ones. A `credentials.json` that is not a credentials
file stops the server from starting, with a message that names it. The uninstall leaves
`credentials.json` in the data folder, as it leaves the store.

## Move issues after a deploy

A project can name statuses after `merged`, such as `deployed` (change its workflow with
`quadratic project update --workflow`). A deploy script on the machine that runs the server
moves the issues of the commit it deployed:

```sh
quadratic issue advance --project DEM --commit "$(git rev-parse HEAD)" --status deployed
```

The command answers `{ "moved": …, "left": … }`, each a count and the first 200 issue ids.

- **Moved:** every issue that is unresolved, is in `merged` or a later status before `deployed`,
  that you may move (an agent cannot move an issue assigned to you, nor one another agent holds),
  and whose every landing is that commit or an ancestor of it. They move to `deployed` in one commit
  of the store, with the operation `issue.advance` and the git commit in its reason.
- **Left:** the unresolved issues before `deployed` that have a landing and did not move: one still
  in review or moved back to `in-progress`, one you may not move, or one that landed again after
  that commit (a later commit with `Refs: DEM-12`): an older deploy never moves newer work. An
  empty `left` means no such issue stayed behind. Issues already in `deployed` or later, and
  resolved issues, are in neither list. Running the same line again moves nothing and answers
  the same `left`: the issues that stayed behind stay behind until they land in a deployed commit
  or someone moves them.
- **Missed:** an issue whose landing the server never saw. It sees a landing when it looks at the
  repository after `git fetch` or a merge; it does not see one from before its first look at the
  project, or a branch deleted before its next look. A squash merge without `Refs: DEM-12` names no
  issue, and a squash merge before the agent's `close` leaves the issue in review. After
  a rewrite of the default branch, an issue whose old landing is no longer in its history is never
  moved. Move such an issue yourself.

The command first looks at the repository, so it sees a merge you fetched a moment ago. That look
records the merges it finds even when the command then refuses. The command fails, and moves
nothing, when the commit is not in your clone or is a tag's id rather than the commit's
(`NotInRepository`: fetch first), when neither the default branch nor its upstream contains it
(`NotOnDefaultBranch`, such as a preview deploy of a branch), when git fails or takes too long
(`ScanFailed`), when the status does not come after `merged` (`NotAfterMerge`), or when the
project is bound to another repository or default branch during the command
(`RepositoryChanged`). A change to the workflow during the command fails it only when the status
stops being valid. The command asks the watcher for a look of its own; that request waits in the
watcher's queue behind any look already running, and the command's write waits for the store.
Neither wait has a limit. Each git command, in that look and in the checks after it, is stopped
after 10 seconds. Only processes of your own account reach this
command; the MCP server of agents does not offer it.

## Import findings from a tool

A tool that reports findings, such as a linter or a checker, records each run with one
`findings import` call. Each finding becomes an issue whose source is the tool and the finding's
`key`. The next complete run finishes the issue of a finding that is gone:

```sh
quadratic findings import --tool linter --findings "$(cat findings.json)" --project ABC
```

Here `findings.json` is a file that your tool writes.

`--findings` takes a JSON array. Each finding has a `key`, a `title` and a `description`, and
can have an `assignee`, a `priority`, `labels`, `links` and `decision`. The type `Finding` in
[`catalog/quadratic.pkl`](catalog/quadratic.pkl) describes every field, and
[`server/tests/fixtures/findings/review.json`](server/tests/fixtures/findings/review.json) is an
example run. A run checks the whole project, so it finishes the issues of the findings it lacks.
For a run that checked only part of the project, pass `--complete=false`: it finishes nothing.
Without `--project`, the project is the one bound to the repository. Only the command line and the web
app can import findings; the MCP server of agents does not offer it.

## Upgrade

Run the same command from the root of an up-to-date checkout:

```sh
moon run quadratic-install:install
```

The install replaces the binary and restarts the service. The new server first stops any other
Quadratic server of the home, even one of an older build. While it does, no `quadratic`
command can start another, so the new server is the one that answers.

The first start of a new binary upgrades the store to that binary's schema, one commit per
step. A binary refuses a store newer than the schema it knows. Back up the store (see "Backups") before you upgrade, so that you can go back to the older binary
with the store as it was.

## Uninstall

```sh
moon run quadratic-install:uninstall
```

This stops the server, checks that no server holds the home, then disables the service and
deletes the unit and the binary. It does not touch the store. If a server of the home does not
stop, or the binary is missing while a server holds the home, the uninstall fails, says why,
and changes nothing.

## Backups

The whole store is one file, `workspace.db`, in `~/.local/share/quadratic/`
(`$XDG_DATA_HOME/quadratic/` when `XDG_DATA_HOME` is set, or the folder `QUADRATIC_HOME` names).
The service uses the data folder that was set when you installed. Run the commands below in a
shell with the same `XDG_DATA_HOME` as at install time and with no `QUADRATIC_HOME` set:
otherwise they act on another store. To back the store up, stop the server, check that it
stopped, copy the file that `server status` names, then start the service again:

```sh
quadratic server stop
quadratic server status      # must say "running": false; note the path under "database"
cp "<the database path from server status>" /path/to/backups/workspace-$(date +%F).db
systemctl --user start quadratic
```

`quadratic server stop` stops the server whether the service or a command started it, and the
service does not restart a server that stopped this way. While the server is stopped, any
`quadratic` command starts it again, so copy the file when no agent is working. There is no
backup command.

To restore, stop the server the same way, check that it stopped, and copy a backup over the
file that `server status` names. Never copy over the file while a server runs: the server then fails with
"database disk image is malformed" and the store does not open, until you stop the server and
copy the backup over it again. The backup file itself is not harmed.

## Develop

The source has these parts:

| Folder | What it is |
|---|---|
| `server` | The Rust library: the store (doltlite), the domain rules, the git watcher and the HTTP server |
| `cli` | The `quadratic` binary: the command line, the MCP server and `quadratic init` |
| `catalog` | The AcidCat catalog in Pkl, rendered to `quadratic.acidcat.json`, which the server and the web app both read |
| `web` | The web app (Vue, Effect, XState) that the server serves |
| `skill/quadratic` | The skill that `quadratic init` writes into a repository; the binary embeds it |
| `install` | The install and uninstall scripts and the systemd user unit |
| `eval` | The evaluation: end-to-end runs and a harness that compares Quadratic with other trackers |

Run every task through moon, for example `moon run quadratic-server:test`:

| Task | What it does |
|---|---|
| `quadratic-server:test`, `quadratic-cli:test` | The Rust tests |
| `quadratic-server:lint`, `quadratic-cli:lint` | Clippy, denying warnings |
| `quadratic-server:fmt-check`, `quadratic-cli:fmt-check` | rustfmt |
| `quadratic-catalog:write` | Renders `catalog/quadratic.pkl` to `catalog/quadratic.acidcat.json`. Run it after each change to the catalog |
| `quadratic-catalog:check` | Fails when the JSON is not what the Pkl renders, or when `acid-cat check --strict` refuses it |
| `quadratic-web:test`, `quadratic-web:e2e` | The web app's headless tests, and its journeys in Chromium against the real binary |
| `quadratic-web:typecheck`, `quadratic-web:lint`, `quadratic-web:format-check` | Static checks of the web app |
| `quadratic-install:test` | Installs on a temporary home with no systemd, and checks the service's cases and the uninstall |
| `quadratic-eval:test` | The evaluation harness's own tests |

### See a change in the web app

The installed binary serves the copy of the web app embedded in it at build time, so it never
shows a change you make. To see one, run a debug build. It is built without embedded assets and
reads the web app from `web/dist` of its source tree at each request.

From the root of the repository:

```sh
moon run quadratic-web:build quadratic-cli:build
```

Then, from `apps/quadratic` (the Rust workspace, the folder with `Cargo.lock`), start that
build's server on a throwaway data folder, so that it neither touches your store nor competes with the installed
server:

```sh
export QUADRATIC_HOME="$(mktemp -d)"
target/debug/quadratic ui --open      # starts this build's server and opens the web app
```

`--open` opens the page with `xdg-open`; without it, `ui` only prints the address.

While it runs, `moon run quadratic-web:dev` rebuilds `web/dist` whenever a source changes; reload
the page to see each build. Stop the server with `target/debug/quadratic server stop`, in the same
shell. A binary of any build serves another folder when the variable `QUADRATIC_WEB` names it in
the environment of the command that starts the server. Give an absolute path: the server runs in
its data folder and reads a relative path from there, so the page would answer 503.

### Releases

[release-please](https://github.com/googleapis/release-please) makes the releases, from the
[Conventional Commits](https://www.conventionalcommits.org) on `main`: a `fix:` makes a patch
release, a `feat:` a minor one (a major one while the version is below 1.0.0). Other commits
make none. The workflow [`.github/workflows/release.yml`](../../.github/workflows/release.yml)
runs on every push to `main`:

1. release-please opens or updates the release pull request. It sets the new version in
   `Cargo.toml`, `Cargo.lock`, the three `package.json` files, `pyproject.toml` and `uv.lock`
   together, and adds the changes to `CHANGELOG.md`.
2. When that pull request is merged, release-please creates the tag `v<version>` and the GitHub
   release.
3. Then one job per platform builds the web app and the release binary with it inside, on the
   oldest hosted Ubuntu (22.04), and attaches the archive and its SHA-256 file to the release.
   The job prints the newest glibc version that the binary needs (2.34). `install/package.sh` makes the archive.

The workflow writes with a token of a GitHub App, not with `GITHUB_TOKEN`: GitHub starts no
workflow for a tag that `GITHUB_TOKEN` pushes, and holds the checks of a pull request that it
opens. It reads the App's client ID from the repository variable `RELEASE_APP_CLIENT_ID` and its
private key from the secret `RELEASE_APP_PRIVATE_KEY`. A pull request that changes the release
workflow, its configuration or `install/` builds the archives too, as workflow artifacts, and
releases nothing.

### The crates

The crates `quadratic-server` and `quadratic` are not published (`publish = false`), but each one
can be packaged as crates.io would take it. Each crate carries the files it reads at build time:
the catalog JSON, the skill and the license are links in the crate to the files of the
repository, and `cargo package` copies their content. The server's crate also carries a copy of
the built web app in `server/web-dist` (ignored by git), which `moon run quadratic-server:stage-web`
writes. A build of the packaged crate with the feature `embedded-web` embeds that copy, as there is
no `web/dist` beside it; a build in the repository embeds `web/dist` as before.

```sh
moon run quadratic-server:stage-web
cd apps/quadratic
cargo package -p quadratic-server --allow-dirty   # --allow-dirty: web-dist is not in git
```

## License

Quadratic is released under the MIT License. See [`LICENSE`](../../LICENSE).
