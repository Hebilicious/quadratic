# Quadratique

Quadratique is a local issue tracker for one human and many AI agents. Agents learn to use it
from the skill that `quadratique init` writes into a repository and from `quadratique instructions`.
This file is for whoever installs, runs or changes Quadratique.

One server owns the store. The `quadratique` command line, the MCP server (`quadratique mcp`) and
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

The browser tests (`quadratique-web:e2e`) also need the Chromium that Playwright drives:

```sh
pnpm --filter quadratique-web exec playwright install chromium-headless-shell
```

The tests also call `jq`, `curl`, `flock`, `systemctl --user` and `unshare --user`, so they need
those commands and unprivileged user namespaces.

On a minimal Linux host, that Chromium also needs system libraries. On Debian or Ubuntu,
Playwright installs them (it asks for `sudo`):

```sh
pnpm --filter quadratique-web exec playwright install-deps chromium-headless-shell
```

## Install

On Linux with systemd, from the root of the repository:

```sh
moon run quadratique-install:install
```

This does four things:

1. Builds the release binary with the web app inside it.
2. Installs the binary as `$XDG_BIN_HOME/quadratique`, or `~/.local/bin/quadratique` when
   `XDG_BIN_HOME` is not set.
3. Writes the systemd user unit `~/.config/systemd/user/quadratique.service`, then enables and
   restarts it. The server then starts with your session. When it crashes or fails to start,
   systemd starts it again after 2 seconds, as often as needed: the unit has no start limit.
   A server that exits with status 5, as it does when its store cannot open, is not restarted:
   `systemctl --user status quadratique.service` shows why it stopped.
   A stop may take up to 70 seconds while the server closes its store; then systemd ends the
   process (`TimeoutStopSec=70`). Every write the server answered is kept.
4. Waits until the unit's main process is the new binary's server and answers. If it is not
   within 60 seconds, or the restart fails, the install fails and prints
   `systemctl --user status quadratique.service`.

Once it is done, the checkout is not needed: you can delete it.

The install refuses to run when `XDG_CONFIG_HOME` is set to anything but `~/.config`. The
systemd user manager looks for units there only when its own environment sets that
variable, so a unit written elsewhere might never start. Run the install with
`XDG_CONFIG_HOME` unset. The install also refuses a binary or data folder whose path has
`"`, `\`, `%`, `$` or a control character, which a unit would read differently.

Check that the server runs, and open the web app:

```sh
quadratique server status
quadratique ui --open
```

The web app is at `http://127.0.0.1:<port>/`. Its TCP listener opens only once a browser was let
in: the first `quadratique ui` opens it, and after that every start of the server opens it again, so
a server that only agents and commands use listens on no TCP port. The server keeps its port across
restarts and upgrades, so open tabs and bookmarks keep working. It keeps the port in `server.port` in its data
folder and asks for it again at each start. When another program holds that port, the server
starts on a port the system chooses and keeps that one; `quadratique ui` prints the current
address, and `quadratique server status` gives it as `web` while the web app listens.
`quadratique serve --port <port>` gives the web app the port you give (`0`: one the system chooses)
and keeps nothing.

The kept address has a risk on a machine with other accounts. While your server is down, one of
them can listen on that port and serve its own page there. If you open a tab or a bookmark of
Quadratique then, that page can read what your browser saved for the address, the token included.
With the token, that account can read and change your issues whenever your server runs, until you
sign out with `quadratique server sign-out`. Its page can also leave a service worker in your
browser that keeps control of Quadratique's tabs at that address, even after your server is back.
If you think this happened: run `quadratique server sign-out`, clear the site data of
`http://127.0.0.1:<port>` in the browser, and turn strict mode on. A machine with only your account
has no such risk. On a machine whose other accounts you do not trust, run `quadratique server strict on`:
from the next start of the server, each start takes an address it never served and a new token, and
you run `quadratique ui --open` after each restart. `quadratique server strict off` goes back to the
default from the next start, which takes a new address and a new token, so each browser is let in
once more with `quadratique ui --open`; after that both are kept. Neither command starts a server.
`server status` and `ui` say `"addressKept": true` only when the next start would ask for the
address the server listens on now: the server kept that port at its start (not with `serve --port`,
and not when `server.port` could not be written; the log says why), strict is off now, and
`server.port` still names that port. With no server running, `addressKept` says what the next start
does.

### Let a browser in

`quadratique ui --open` opens the web app and lets this browser in: run it once per browser, and
there is nothing to type or copy. The browser keeps a token for the web app, and its tabs and
bookmarks keep working across restarts and upgrades. A browser that was never let in shows one
sentence: "To open Quadratique in this browser, run `quadratique ui --open` in a terminal."

How it works: `ui` asks the server, over its socket, for a one-time code, valid once and for 60
seconds. With `--open` it writes the link into a file of the data folder that only your account can
read (`open-<random>.html`) and opens that file, so the code never appears in a process list. The
server removes the file once the code is used or has expired, and at its next start. Without
`--open`, `ui` prints the link as `link`, for a browser you start yourself or an SSH forward.

The page takes the code out of its address before anything else runs, and exchanges it at
`POST /session` for the token, which it saves in the browser and sends as `Authorization: Bearer`
on every request and on its live channel.

`ui --open` answers `"opened": true` once the opener (`xdg-open`, or `open` on macOS) ran and
exited without an error; that does not prove a browser read the file. A browser in a sandbox
(Firefox as a snap, a Flatpak browser) usually cannot read the data folder, and `xdg-open` may
hand an `.html` file to an editor. When the browser shows no web app, run `quadratique ui` and
open the `link` it prints, within 60 seconds.

The server keeps the token in `web-token` in the data folder, mode `0600`. To sign every browser
out, run `quadratique server sign-out`: it makes a new token, and every live channel opened with
the earlier one delivers nothing more once the command answers and closes, so each open page
shows the sentence at once, and every other browser at its next request, until you run
`quadratique ui --open` there again. In strict mode (`quadratique server strict on`) the token is
made at each start of the server and kept in memory only, so every restart signs every browser
out. A strict setting that you change applies from the next start of the server.

Every request of the web app and its live channel needs the token: a request without it, or with
an earlier one, gets `401`, and the page shows the sentence.

### Use it over SSH

The web app listens on the loopback address of the machine that runs the server. To use it from
your own machine, forward the same port before you open the link:

1. On the remote machine, run `quadratique ui` (without `--open`). It prints `url`, with the port,
   and `link`, with a code valid for 60 seconds.
2. On your machine, run `ssh -L <port>:127.0.0.1:<port> <host>` with that port.
3. Open the `link` in your browser within the 60 seconds. If the code expired, run `quadratique ui`
   again.

Use the same port on both ends: the web app answers only under its own address,
`127.0.0.1:<port>`. The forward runs as you on the remote machine, so everyone who can connect to
its local end reaches the web app as you would; the page still needs the token. Do not forward the
socket (`ssh -L ./q.sock:<data folder>/server.sock`) unless you mean it: whoever can connect to the
forwarded socket can call every command as you, with no token.

## Install from a release

Each [GitHub release](https://github.com/Hebilicious/quadratique/releases) has a prebuilt binary,
with the web app inside it, for Linux on x86-64 and on arm64. It needs glibc 2.34 or newer and
`libgcc_s.so.1` (Ubuntu 22.04, Debian 12, RHEL 9, Fedora 35 or later have both), and no other
tool: no Rust, Node.js or proto. The service that `install.sh` sets up needs a running systemd
user manager (`systemctl --user status` answers), as for an install from source.

Each platform has one archive, named for the version and the target, and its SHA-256 file:

| Platform | Archive |
|---|---|
| Linux x86-64 | `quadratique-<version>-x86_64-unknown-linux-gnu.tar.gz` |
| Linux arm64 | `quadratique-<version>-aarch64-unknown-linux-gnu.tar.gz` |

The archive has one folder, `quadratique-<version>-<target>/`, with the binary in `bin/quadratique`,
the scripts `install.sh` and `uninstall.sh`, this file and the license. `install.sh` is the script
that `moon run quadratique-install:install` runs, and does the steps 2 to 4 of "Install" with the
binary you give it. Download, check and install a release (here 0.1.0 for x86-64; use `aarch64`
for arm64):

```sh
version=0.1.0
archive=quadratique-$version-x86_64-unknown-linux-gnu
curl -fLO "https://github.com/Hebilicious/quadratique/releases/download/v$version/$archive.tar.gz"
curl -fLO "https://github.com/Hebilicious/quadratique/releases/download/v$version/$archive.tar.gz.sha256"
sha256sum -c "$archive.tar.gz.sha256"
tar -xzf "$archive.tar.gz"
sh "$archive/install.sh" "$archive/bin/quadratique"
```

To upgrade, run `quadratique upgrade` (see "Upgrade"). To uninstall, run `sh "$archive/uninstall.sh"`
from any release: it removes the binary and the unit that the unit names.

### With mise

[mise](https://mise.jdx.dev) installs the binary from the GitHub releases. It picks the archive
for your platform from its name, checks it against the SHA-256 file, and unpacks the archive's
folder. It puts that folder's `bin/` on your `PATH`, so `quadratique` is the only command it adds;
`install.sh` and `uninstall.sh` stay in the folder:

```sh
mise use -g github:Hebilicious/quadratique
```

This installs only the binary. To run the server as a systemd user service, also run the
`install.sh` of that release with the binary that mise installed:

```sh
sh "$(mise where github:Hebilicious/quadratique)/install.sh" "$(mise which quadratique)"
```

The install copies the binary to `~/.local/bin/quadratique`, so the service keeps the version you
installed until you run `install.sh` again after `mise upgrade`. `quadratique upgrade` prints that
mise command instead of replacing the binary. By default mise does not install a
release published less than 24 hours ago (its setting `minimum_release_age`); name the version to
install it earlier (`mise use -g github:Hebilicious/quadratique@0.1.0`).

### With cargo

From the first publication to crates.io on (not before), cargo builds and installs the binary,
with the web app inside it:

```sh
cargo install quadratique
```

This builds from source, so it needs Rust, a C compiler and the zlib development files (the store
engine is compiled from C; on Debian or Ubuntu: `sudo apt install build-essential zlib1g-dev`).
Then run the `install.sh` of the release of the same version, with `~/.cargo/bin/quadratique`, to
set up the service.

## Set up a repository

From inside a git repository:

```sh
quadratique init --key ABC --name "My project"
```

`init` writes the skill to `.agents/skills/quadratique/` (`SKILL.md` and `planning.md`), adds a
line to `AGENTS.md` that points agents to the skill, and creates the project bound to the
repository. Without `--key`, the key comes from the folder's name. The project's default branch
comes from `--default-branch`, else from the remote's `HEAD`, else from the branch checked out in
the main worktree. When none of these names a branch, as in a main worktree with a detached
`HEAD` and no `origin/HEAD`, the project gets `main`, and the answer's `defaultBranchFrom` says
"nothing in the repository names one: give --default-branch". Change it later with
`quadratique project update`.

A skill file you changed is left as it is: `init` replaces only a file that holds a text an
earlier build wrote (see "Update a repository's skill after an upgrade"). Of the setup files, `init` writes only the three
above, all inside the repository, and writes no agent configuration. Like any command, it starts
the server when none answers, and the server keeps its store in the data folder (see "Backups"),
outside the repository. `init` makes no commit. Its answer has an optional `commit` field: the
`git add` and `git commit` to run for the setup files that `init` wrote, or that hold exactly its
own text and are not committed yet. A skill file you changed, which `init` left alone, is not in
that command. When every setup file is already committed, the answer has no `commit` field.

## Update a repository's skill after an upgrade

A new build can ship a new skill text. Bring a repository's setup to it with the same command,
from inside the repository:

```sh
quadratique init
```

The binary knows every skill text that a build of Quadratique wrote, by the SHA-256 of its bytes.
A skill file that holds one of them is replaced with this build's text, and the answer says
`updated: it held an earlier Quadratique text`. A skill file that holds any other bytes was changed
by a person: it is kept, and the answer says `different: yours is kept`. `init` replaces a file by
writing a new file beside it and renaming it over the old one, so another hard link to the old
file keeps its bytes. It checks the file again just before the write: a file that changed since
`init` read it, or one that appeared where there was none, is kept with a warning.

A repository set up by Quadratic, the earlier name, moves to Quadratique: the skill is written to
`.agents/skills/quadratique/`, the files of `.agents/skills/quadratic/` that hold a text Quadratic
wrote there are removed, and Quadratic's line in `AGENTS.md` becomes Quadratique's. The `commit`
field records these changes, also on a later run, as long as git's index still holds Quadratic's
text at the removed path (the answer then says `removed earlier: the commit records it`).

In `AGENTS.md`, the Quadratique line is a line that is exactly the instruction from its first
character, followed by nothing but spaces or tabs, outside any fenced code block. A quotation, an
indented line and an example inside a fence are left as they are. The first standalone Quadratic
line becomes the Quadratique line, unless that is already there; other Quadratic lines are removed.
Every other byte of the file stays as it is, line endings and a missing final newline included. A
file without the line gets it at its end, in the file's own line ending.

`quadratique init --dry-run` writes nothing: it creates no project, starts no server and runs no
git command that writes. It asks a server about the project only when one already runs; otherwise
its `project` is `{"lookedUp": false, "reason": "..."}`. Its answer has `"dryRun": true` and says per
file what `init` would do: `would create` (the file is missing), `unchanged` (it holds this build's
text), `would update: it holds an earlier Quadratique text`, `different: yours is kept` (a person
changed it), or `would fail: <reason>` when the file or its folder is not writable.

A kept file is also a warning. The answer then starts with a `warnings` list that names each
kept file and the command that prints this build's text of it, for example:

```json
{"warnings":[".agents/skills/quadratique/SKILL.md was kept: a person changed it, so it lacks this build's text. Merge that text into it: quadratique init --print SKILL.md"],"files":{...}}
```

`quadratique init --print SKILL.md` writes this build's `SKILL.md` to standard output as it is, not
wrapped in JSON, so that you or an agent can compare it with the kept file and merge the two.
`--print planning.md` gives the other skill file and `--print AGENTS.md` the line. `--print` needs
no repository and no server and writes nothing, so it works in a sandbox that cannot reach the
server. Any other name is refused with the list of names it knows.

`init` removes a file of `.agents/skills/quadratic/` only when it is a regular file, reached
through no link, that holds exactly a text Quadratic wrote there before the rename. It follows no
link there: a link, a directory, a changed file (a copy of a later Quadratique text included) and
any other file are kept, each with a warning, and the folder is kept when it is not empty. For a
changed file, the warning says where the merged text goes: `.agents/skills/quadratique/`, which
Quadratique reads.

## Who can talk to the server

The server listens in two places, each with its own rule.

The commands and `quadratique mcp` use the Unix socket `server.sock` in the data folder. File
permissions decide who connects there: the socket has mode `0600` in a folder of mode `0700`, so a
process of another account cannot reach it, while `root` passes those permissions and can call the
server like you. The server asks nothing more of a process on the socket.

The TCP listener on `127.0.0.1` serves only the web app, and opens only once a browser was let in
(see "Let a browser in"). It answers only requests that name its own numeric address
(`Host: 127.0.0.1:<port>`); any other name, `localhost` included, gets `403`. Anyone who reaches it
gets the app's pages and files, and only a request with the token gets data: `/web`, `/webmcp` and
their live channel answer `401` without it. The command line's routes and the control requests
(`/cli`, `/server` and below) answer `404` there. On Linux it also answers only processes of the
account that runs it. At each connection it asks the kernel which account owns the caller's socket
(`NETLINK_SOCK_DIAG`). A process of any other account, `root` included, gets `403` "This server
answers only processes of its own account." That `403` is the TCP listener's rule only. A server that cannot ask the kernel who is
connecting, in a sandbox that forbids netlink or on a kernel without its `inet_diag` module, does
not start and says why. So does a server that runs in a user namespace where its account is not
mapped, where the kernel would show every unmapped caller as that same account. If a lookup
fails once the server runs, the server refuses that connection and says so once in its log.
Every process of your own account is you, whatever name it gives. The rule sees only the process at the other end of the connection: a proxy or port
forward that runs as you (`ssh -L`, socat, the userland forwarders of rootless containers) is
that process, so everyone who connects through it counts as you.

The data folder must be private. The server and every command check it before they open a lock
file in it, touch the store or start a server. When the check refuses the folder, the command
exits with status 5 and says which rule failed and what to do. Nothing is changed for you: the
check never runs `chmod`. Each cause has its own fix:

| The message says | Do this |
| --- | --- |
| `<folder> is not private: its mode is 755 …` | `chmod 700 <folder>`. Quadratique 0.1 made the folder private itself; this release refuses a folder that a group or other accounts may use, and leaves it for you to change. |
| `<folder> gives access to another account: its ACL lets …` | Remove the ACL entry, not the mode. On Linux: `setfacl -b <folder>` (its entries), or `setfacl -k <folder>` (its default ACL). On macOS: `chmod -N <folder>`. The message names the command. |
| `another account can replace <folder above it> …` | That folder belongs to another account, or a group or other accounts may write it without the sticky bit. Remove their write access (`chmod go-w <folder>`) if the folder is yours, or set `QUADRATIQUE_HOME` to a folder that only you and root can change, such as one in your home folder. |
| `<link> is a link that another account could change …` | Name the folder without that link, or set `QUADRATIQUE_HOME` to a folder whose path has no link that another account owns. |
| `the server's socket would be … bytes long …` | Set `QUADRATIQUE_HOME` to a shorter folder. The path of `server.sock` may be at most 107 bytes on Linux and 103 on macOS. |
| `<folder>/server.sock is a regular file …` | Remove that entry yourself if nothing needs it. The server removes only a socket of your account on which no process listens any more. |

A missing data folder, and any missing folder above it, is made with mode `0700`.

The server also listens on the Unix socket `server.sock` in the data folder, with mode `0600`.
`server status` asks it there first, and reports one state: `running`, `starting` (a server holds
the folder and does not answer yet), `stalled`, `malformed`, `otherDatabase`, `newer` (upgrade this
command line), `reachable: false` with the reason, or `legacy: true` for a server from before the
socket (release 0.1), which only TCP reaches. `server stop` and `serve --take-over` ask the server on
the socket to stop, and never signal it. Every other command, `quadratique mcp`, `init` and
`upgrade --skills` call the server on the socket too, and never over TCP: the command finds the
server there first, as `server status` does, and only then sends its call through the same socket.
So file permissions decide who can use the server from the command line: the socket's mode `0600`
in a folder of mode `0700`. No server writes `server.json` any more, and no route of the command
line answers on TCP: a tool that called `/cli` over TCP calls the socket instead
(`curl --unix-socket <data folder>/server.sock http://quadratique.invalid/cli/...`).

### A server of release 0.1

A server of release 0.1, or of any build before the socket, holds the data folder's lock, has no
socket, and leaves a `server.json` without a `control` field. No command that needs the server uses
it: each one fails with "Nothing was started." and says what to run. `upgrade --skills` reads the
store file instead, `init --dry-run` says that it asks no server, and `upgrade` stops nothing and
says what to stop (see "Upgrade"). `server status` answers `legacy: true`, the pid that
`server.json` names, `provenStop`, the `reason`, and in `stop` what to run. `provenStop` is the
evidence about the process; a stop still has to reserve `start.lock`, and checks the evidence again
under it.

The proof exists so that no process that is not this data folder's old server is ever signalled by
mistake: a stale `server.json`, a reused process number, a wrong path, another program. A process of
your own account that deliberately imitates the old server is out of scope: it already has your
files. Anything can answer on a TCP port, and a pid alone proves nothing, so `server stop` and
`serve --take-over` signal that server only with this proof, on Linux:

1. They open a pidfd on the pid that `server.json` names.
2. `/proc` numbers processes as the command's own PID namespace does: `/proc/self` is the command's
   own pid, and the pidfd's `Pid:` in `/proc/self/fdinfo` is the pid it was opened by.
3. The process runs the installed binary, `${XDG_BIN_HOME:-$HOME/.local/bin}/quadratique`, where
   `install.sh` of release 0.1 put it. It counts only as a regular file of your account, reached
   without a link, in a folder that `upgrade`'s folder rule accepts, and never as the command line's
   own file (`/proc/self/exe`).
4. One of its descriptors of the folder's `server.lock` holds the exclusive `flock`, as
   `/proc/<pid>/fdinfo/<fd>` lists it (Linux 4.13 and later; where the kernel lists no locks, there
   is no proof). A descriptor of the same file that holds no lock does not count.
5. The process is still alive after these reads, so no other process can have had its number.

They look at this before they reserve `start.lock`, so a refusal leaves nothing behind. Under
`start.lock`, right before the signal, the folder must still show this row, `server.json` must still
name that process, and the process must still run that file and hold that lock. A stop that first
found a server on the socket stops only that generation. Then they send SIGTERM through the pidfd and
wait up to 30 seconds for the folder to be free. A verified server could still `exec` another program
between the last check and the signal: release 0.1's server never does, and only your own account
could make it.

Without the proof (another binary, a binary that was replaced since it started, a link, no lock, a
`/proc` of another namespace, another system) they signal nothing, and the message says what to run:

```text
Stop it yourself, then run the command again: if `quadratique.service` runs it (`systemctl --user status quadratique.service` names its main PID), run `systemctl --user stop quadratique.service`; if it runs in a terminal, press Ctrl-C there; otherwise check pid <pid>, which nothing has verified, with `ps -p <pid> -o args`, and only if it shows this data folder's Quadratique server, end it with `kill <pid>`.
```

The process commands appear only for a valid pid other than the command's own; `kill 0` would
signal your own process group. A server whose binary was replaced since it started (by `install.sh`,
mise or a copy) runs a file that no longer has a name, so it gets these instructions. That is never
a file that `quadratique upgrade` replaced: an upgrade replaces the binary it runs from, and release
0.1 has no `upgrade`, so a record of the files it replaced would prove nothing about a server of
release 0.1, and none is kept for it.

Release 0.1 was published for Linux only, but its code refuses no system. On macOS the command line
reads no `/proc`, so it always gives these instructions.

A server that answers `POST /server/stop` with a status other than `202` or `409` is not stopped:
`server stop` names the status and says how to end that server yourself (Ctrl-C in its terminal,
its supervisor, or `ps -p <pid> -o args` first). For `405`, it adds that the server may be a build of
`main` between the socket and the stop request (never released).

### The control protocol

The socket serves the command line's routes under `/cli`, as the commands call them (for example
`curl --unix-socket "$QUADRATIQUE_HOME/server.sock" http://quadratique.invalid/cli/capabilities`),
and two control requests, with any `Host` (use `quadratique.invalid`). A client must send each
request whole, head and body, within 5 seconds of opening the connection or of the end of the
previous answer, or the server closes the connection. An answer has no limit once its request has
arrived, because a call may wait and `watch` keeps its stream open. The TCP listener has no such
limit. Each server start has a
**generation**, 32 hexadecimal digits that are new at each start. The protocol has a version,
`control`, apart from the interface of the commands. A server keeps answering every control version
it ever served, and lists them in `controls`. Two fields keep their meaning in every version:
`generation` and `database`. A command line checks them before it looks at the control versions, so
it stops a newer server only in its own folder and only by its generation. This is version 1:

| Request | Answer |
| --- | --- |
| `GET /server` | `200` and the identity: `pid`, `database`, `interface`, `schema`, `generation`, `control`, `controls`, `executable` (`path`, `device`, `inode`), `started` (`on-demand` or `serve`), `web` (`http://127.0.0.1:<port>/`, while the web app listens) and `addressKept` with it, and `url`, always `http://127.0.0.1:0/cli`: the command lines of releases 0.2 and 0.3 accept an identity only with a loopback address of the command line's routes there, and no route answers at it. |
| `POST /server/stop` with `{"generation": "<generation>"}` | `202 {"generation": …}` if it names this server's generation: the server has closed its listeners, and starts to shut down once this answer is sent. Open connections get up to 2 seconds, then the server closes its store and frees the folder. `409 {"generation": <the serving one>}` for any other generation: nothing stops. `400` for a body that is not that JSON. |

```sh
curl --unix-socket "$QUADRATIQUE_HOME/server.sock" http://quadratique.invalid/server
curl --unix-socket "$QUADRATIQUE_HOME/server.sock" -X POST \
  -d '{"generation":"<generation>"}' http://quadratique.invalid/server/stop
```

`server stop` reads the generation, asks that generation to stop, and waits up to 30 seconds for
the folder to be free. It answers `"stopped": true` with the `pid` and the `generation` it stopped.
The generation it reads first is the one it stops. If it must wait for `start.lock` and another
generation serves when it gets the lock, it stops nothing and names both. If the first look found a
server that was still starting, it stops the first generation that becomes ready. If the stop
request was sent and its answer was lost, `server stop` says the stop is not confirmed and whether
the folder's lock is still held. Run `server status` to see what serves then.
A server that does not answer, or does not let go in time, is never signalled: the message names
the folder, the log and the last known pid, and says how to end it yourself. Every start and every
stop holds `start.lock` in the data folder, so no command starts a server between a stop and the
start that follows it. A command never replaces a server that stalls, answers something malformed,
serves another database or speaks a newer control version: each has its own message.

If a user service runs the server, a refusal stops it: the unit does not restart a server that
exits with status 5 (`RestartPreventExitStatus`). Fix the cause, then
`systemctl --user restart quadratique.service`. `quadratique upgrade` checks the folder before it
looks for a release, and stops there with the reason and "Nothing was changed".

Some agent sandboxes forbid connections to the server's socket, hide the data folder, or make the
file system read-only, so that a command can neither reach the server nor start or stop one in the
data folder. A command run in such a sandbox, or by another account, fails, and starts, creates and
stops nothing:

```text
Cannot reach the Quadratique server from here: <reason>. Nothing was started. Call the Quadratique MCP tool `<tool>` with the same inputs instead. The MCP tools act from the folder this session started in; for work in another worktree, pass its absolute path as `worktree` to `issue.claim`; once that claim succeeds, later calls act from it. If this session has no Quadratique MCP tools, tell the human that its harness must start `quadratique mcp` as an MCP server.
```

The reason is the one the command met, for example "connecting to
/home/you/.local/share/quadratique/server.sock is not permitted", "this process cannot use
/home/you/.local/share/quadratique: Permission denied; a sandbox may be blocking it" (a sandbox
that hides the folder, or another account whose home folder is closed to it) or "this process
cannot write /home/you/.local/share/quadratique: Read-only file system; a sandbox may be blocking
it". Every command that would reach the server, `quadratique mcp` included, exits with status 4
(`unavailable`) for each of them, and `quadratique server status` answers `"reachable": false` with
the reason. `quadratique serve` is the server itself, not a caller: a data folder it cannot use is
a failure of its start, with status 5. This is the answer of a command whose capability is also
an MCP tool, and `<tool>` names that tool. Every other command that needs the server ends with
"Nothing was started. This command works only on the command line: run it outside the sandbox,
or ask the human to." Input the command line cannot match to a command (a typo after a group
word, a bare group, a flag other than `--json` before the command) ends with "Nothing was started. `quadratique --help`
lists the commands." `quadratique ui` and `quadratique serve --take-over`, which people run, end with
"Nothing was started." and no advice. `quadratique server stop` ends with "Nothing was stopped." and no advice. `quadratique server
status` answers `"reachable": false` and the reason. `quadratique mcp` checks at its start and
exits on standard error with "Nothing was started.", before it offers any tool; if the server
becomes unreachable later, it writes each reason once to standard error. "Agents in a sandbox"
below says how to let such an agent work.

## Agents in a sandbox

Where an agent's shell runs decides whether the `quadratique` command works there:

| Where the command runs | What happens |
| --- | --- |
| A Claude Code shell without a sandbox | Every command works: it reaches the server on its socket, as in your own terminal. |
| Codex's sandbox, default policy | No command that needs the data folder works; `--help` and `--version` answer. The sandbox refuses to make a Unix socket (`Operation not permitted`), and before that, Quadratique refuses the data folder: the sandbox shows every folder of root, such as `/home`, as owned by the account 65534, so the command answers "another account can replace /home: it belongs to another account (uid 65534) …" and exits with status 5 (QUA-84). |
| Codex's sandbox with `network_access = true` | The socket and the loopback interface both connect, but the command still meets the same refusal of the data folder, and exits with status 5 (QUA-84). |

Codex starts its MCP servers outside the sandbox, so a Codex session can use Quadratique through
`quadratique mcp`. A command whose sandbox blocks the socket, hides the data folder or makes it
read-only answers "Cannot reach the Quadratique server from here", names the MCP tool to call
instead, and tells the agent to ask you for the tools when the session has none. A command that
has no MCP tool says that it works only on the command line, to run outside the sandbox.

Give Codex the tools once, for every session of your account:

```sh
codex mcp add quadratique --env AI_AGENT=codex -- quadratique mcp
```

This writes `[mcp_servers.quadratique]` with `command`, `args` and `env` to `~/.codex/config.toml`.
Codex does not pass its own environment to an MCP server, so Quadratique cannot tell that the
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

Letting the sandbox reach the network (`network_access = true` under `[sandbox_workspace_write]`
in `~/.codex/config.toml`) does not make the command work in it today, as the table says, and it
lets every sandboxed command of every Codex session reach the network. Use the MCP tools.

The table was measured on 2026-11-26 on Linux 7.2 with Claude Code 2.1.332, and with codex-cli
0.172.0 through `codex sandbox`, which runs a command under the sandbox policy that a session
uses, with Quadratique 0.2.0 from the main branch and with this release; both builds gave the same
outcome in each place. These facts were measured earlier with codex-cli 0.171.0 in a Codex
session and not again: the MCP tools working in a sandboxed session while the command was
blocked, the project taken from the session's folder, the actor names with and without
`AI_AGENT`, and what `codex mcp add` writes. A `.codex/config.toml` inside a project folder was
not listed by `codex mcp list`, so this section names only the file in your home.

## Agent tokens

An agent token names one agent, by a label, for an agent that will work from another machine
than the one that runs the server. No route accepts a token yet.
Only the command line manages tokens: no web page and no MCP tool can.

```sh
quadratique token create --label deploy --file /home/you/secrets/deploy.token
quadratique token list
quadratique token revoke --label deploy
```

`create` writes the token's value, `qdt_` followed by 43 characters, to the file that `--file`
names. The path must be absolute, and nothing may be at it yet: the server creates the file with
mode `0600` and refuses a path where a file or a link already is (`FileExists`), and a relative
path (`NotAbsolute`). The file cannot be in the data folder itself (`InDataFolder`), where the
server's own files would overwrite or remove it; a link to that folder counts as the folder. The value is in that file and nowhere else: not in the command's output,
not in the server's log, not in the store. The label is the agent's name. It follows the rule of a
role: lowercase letters, digits and single hyphens, at most 32 characters. Two tokens cannot share
a label, and no token can take the name of one of the server's own actors, `quadratique`, `agent`,
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
`quadratique project update --workflow`). A deploy script on the machine that runs the server
moves the issues of the commit it deployed:

```sh
quadratique issue advance --project DEM --commit "$(git rev-parse HEAD)" --status deployed
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
  issue, and a squash merge before the agent's `finish` leaves the issue in review. After
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
quadratique findings import --tool linter --findings "$(cat findings.json)" --project ABC
```

Here `findings.json` is a file that your tool writes.

`--findings` takes a JSON array. Each finding has a `key`, a `title` and a `description`, and
can have an `assignee`, a `priority`, `labels`, `links` and `decision`. The type `Finding` in
[`catalog/quadratique.pkl`](catalog/quadratique.pkl) describes every field, and
[`server/tests/fixtures/findings/review.json`](server/tests/fixtures/findings/review.json) is an
example run. A run checks the whole project, so it finishes the issues of the findings it lacks.
For a run that checked only part of the project, pass `--complete=false`: it finishes nothing.
Without `--project`, the project is the one bound to the repository. Only the command line and the web
app can import findings; the MCP server of agents does not offer it.

## Upgrade

```sh
quadratique upgrade
```

`upgrade` asks the GitHub API for the latest release and compares it with the running binary.
When the release is newer, it:

1. checks that the store can be opened by the release (see below), that the binary is not inside
   a git working tree, and that no other account can replace files in its folder: the folder and
   every folder above it belong to you or to root, no group and no other account can write the
   folder, and a folder above it may be writable by a group or by others only when it is sticky,
   as `/tmp` is;
2. takes a lock in that folder (`.quadratique-upgrade.lock`), held until the server is stopped
   and the repositories are reported, so that one upgrade at a time works on the binary;
3. downloads the archive for this platform from github.com, which redirects to
   `release-assets.githubusercontent.com` (or `objects.githubusercontent.com`,
   `github-releases.githubusercontent.com`); no other host is reached, and only over HTTPS;
4. checks the archive against the SHA-256 that the GitHub API gives for it (`digest`);
5. writes the new binary into a private folder beside the running one, runs that very file (on
   Linux through its open descriptor) with `--version` in an empty environment from `/`, checks
   the store schema again, and renames the binary over the running one.

Then it stops the server of this home if that server ran the replaced file, so that the next
command starts the new build. It finds the server on the home's socket, as every command does,
never over TCP, and it signals no process:

1. Before the rename, it takes `start.lock` and reads the server's identity on the socket (see
   "The control protocol"): its `generation` and its `executable`, the device, inode and path of
   the file it was started from. It holds `start.lock` through the rename and the stop, so no
   server starts in between unseen.
2. After the rename, a server whose `executable` is the replaced file is asked to stop that
   generation (`POST /server/stop`). If the answer is `409`, another generation serves now, and
   nothing is stopped: the answer says so. A server that runs another file (another install) is
   left running, and `server` says which file it runs.
3. A server started with `quadratique serve` is stopped too, and the answer (in `warnings`) says to
   restart it the way you started it: your supervisor or your terminal.
4. When `quadratique.service` runs that server (its main process is the generation's process), the
   upgrade stops the generation the same way, gives up `start.lock` (the unit's own start takes
   it), waits until systemd sees the process leave, and starts the unit again: the unit runs
   `serve --take-over` of the new binary. It then waits until the server on the socket is the
   unit's main process and says it runs the installed file. The stop, the unit's start and that
   wait share one deadline of 60 seconds. A command that starts a server between the stop and the
   unit's start gets a server that the unit's take-over then replaces: the answer names each such
   generation that the upgrade saw. If the unit changed on disk since systemd loaded it, or
   `systemctl` fails, nothing is stopped and the answer says what to run.
5. A server from before the socket (release 0.1) is never stopped by `upgrade`. The answer says
   what serves and what stops it: `quadratique server stop` when that command has its proof (see
   "A server of release 0.1"), otherwise the steps to stop it yourself. A file that `upgrade` replaced never counts as
   that proof: the upgrade replaces the binary it runs from, and release 0.1 has no `upgrade`, so
   no server of release 0.1 runs a file that an upgrade replaced.

Its answer says the version it left (`from`), the one it reached (`to`), and in `server` what it
did, or in `warnings` what you must do. Run again, it answers `"upToDate": true` and changes
nothing. When an earlier run was stopped between the rename and the stop, the next run finishes
it: the lock file holds the device and inode of the file that run replaced, and the next run stops
the server that says, on the socket, that it was started from the binary's path and from that file
while the path now holds another. It reads that record only when no other account can change the
install folder. If an earlier run stopped the service's server and was stopped before it started
the unit again, no server runs and the unit is inactive. The next run then starts nothing, because
you may have stopped the unit on purpose. When the unit is loaded, not active, and starts this
binary, the answer says so and gives the command: `systemctl --user start quadratique.service`.
`quadratique upgrade --check` says the installed and the latest version and changes nothing: it
creates neither a missing data folder nor the install lock.

A refusal before the upgrade takes its lock ends with "Nothing was changed.". From the lock on, the
lock file exists and leftovers of an earlier run may be removed, so a refusal ends with "The
installed binary was not changed.".

Any failure before the rename leaves the running binary in place, and the error says so (see above
for its two endings): a download that fails, an archive that does not match its digest, a release without
an archive for this platform, an archive with an entry outside its folder, a new binary that does
not run or says another version. `upgrade` never installs an older release than the running one
unless you pass `--allow-downgrade`, and does not run as root.

Each binary knows the store schema it opens. A newer release opens every schema the running build
knows. For an older release, the binary keeps a table of the first release of each schema; a
downgrade is refused when the store has a schema the release does not know: its server could not
start. The schema comes from the home's server when one answers on the socket, else from the store
file.

When mise or cargo installed the binary, `upgrade` replaces nothing. It answers that manager's
command in `run` (`mise upgrade --bump github:Hebilicious/quadratique`, or
`cargo install --root <root> quadratique`) and, after it, `quadratique upgrade --skills`. It tells
them from where the binary is: inside a mise `installs/<tool>/<version>/` folder whose
`.mise.backend.toml` names this repository, or in a `bin/` folder beside a `.crates2.json` that
lists the `quadratique` package. When a sign of a manager does not hold up (a marker that cannot be
read or names another tool), it replaces nothing and says why.

`upgrade` defends against other accounts on the machine and against the network. A process of
your own account already holds your files and is outside what it defends against. The digest
comes from the GitHub API, not from the place that serves the archive. What is still trusted:
GitHub, and whoever can publish a release of `Hebilicious/quadratique`.

The first start of a new binary upgrades the store to that binary's schema, one commit per
step. A binary refuses a store newer than the schema it knows. Back up the store (see "Backups") before you upgrade, so that you can go back to the older binary
with the store as it was.

### `close` is now `finish`

Release 0.2 renames the command that hands finished work to review, with its evidence:
`quadratique close` is now `quadratique finish`, the capabilities `issue.close` and `issues.close`
are now `issue.finish` and `issues.finish`, and the refusal `NotClosed` of several issues is now
`NotFinished`. On GitHub, closing an issue without a merge drops it; here the work is done and
waits for review, so the new word says that.

The old names still work in 0.2 and go away in the release after it. `quadratique close`,
`quadratique issue close`, `quadratique issues close` and `quadratique call issue.close` do what
they did and answer what they answered, `NotClosed` included; after the answer or the refusal,
which go to standard output, they add one line on standard error:
`` quadratique: `close` is now `finish`; `close` goes away in the release after 0.2. ``. Over
HTTP, `issue.close` and `issues.close` answer as before. The MCP tool is `issue.finish` only: an
MCP client reads the tools from the server at each start, so an agent whose MCP session was open
during the upgrade restarts it to see `issue.finish`.

Every finish now writes the history operation `issue.finish`, whichever name was called. History
written before 0.2 keeps `issue.close`; `history query` shows both as they were written, and the
web app reads both as "finished with evidence". An older build shows `issue.finish` as it is
written, in `history query` and in its web app. Update each repository's skill with
`quadratique init` (see "Update a repository's skill after an upgrade"); scripts that call
`quadratique close` need `finish` before the release after 0.2.

### The repositories after an upgrade

`upgrade` writes into no repository, because a repository may take changes only through a branch
and a pull request. Its answer has a `skills` report, which the new binary makes. It starts no
server: when one answers on the home's socket, the server lists the projects; otherwise (no
server, or one from before the socket), when a store
exists, the command reads it once, read-only, and the store engine may leave its lock file
(`.workspace.db-lock`) beside it, which the engine creates for any reader. Then it reads the main
checkout of each project's repository, as `quadratique init --dry-run` does. It lists

- `current`: the repositories whose skill is this build's;
- `older`: each repository whose skill holds an earlier build's text, with the command to run
  there (`cd <repository> && quadratique init`, see "Update a repository's skill after an
  upgrade");
- `kept`: each repository with a skill file that a person changed, with a warning that names the
  file and the command that prints this build's text of it (`quadratique init --print SKILL.md`),
  for an agent to merge with the custom instructions;
- `noSkill`: the repositories whose main checkout holds no skill. When a worktree or another
  branch holds it, run `quadratique init --dry-run` there;
- `bare`, `missing` and `errors`: bare repositories, repositories that are no longer on this
  machine, and what could not be read.

`quadratique upgrade --skills` makes the same report, without the network.

To build from an up-to-date checkout instead, run `moon run quadratique-install:install` from its
root: it replaces the binary and restarts the service.

## Uninstall

```sh
moon run quadratique-install:uninstall
```

This stops the server, checks that no server holds the home, then disables the service and
deletes the unit and the binary. It does not touch the store. If a server of the home does not
stop, or the binary is missing while a server holds the home, the uninstall fails, says why,
and changes nothing.

## Backups

The whole store is one file, `workspace.db`, in `~/.local/share/quadratique/`
(`$XDG_DATA_HOME/quadratique/` when `XDG_DATA_HOME` is set, or the folder `QUADRATIQUE_HOME` names).
The service uses the data folder that was set when you installed. Run the commands below in a
shell with the same `XDG_DATA_HOME` as at install time and with no `QUADRATIQUE_HOME` set:
otherwise they act on another store. To back the store up, stop the server, check that it
stopped, copy the file that `server status` names, then start the service again:

```sh
quadratique server stop
quadratique server status      # must say "running": false; note the path under "database"
cp "<the database path from server status>" /path/to/backups/workspace-$(date +%F).db
systemctl --user start quadratique
```

`quadratique server stop` stops the server whether the service or a command started it, and the
service does not restart a server that stopped this way. While the server is stopped, any
`quadratique` command starts it again, so copy the file when no agent is working. There is no
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
| `cli` | The `quadratique` binary: the command line, the MCP server and `quadratique init` |
| `catalog` | The AcidCat catalog in Pkl, rendered to `quadratique.acidcat.json`, which the server and the web app both read |
| `web` | The web app (Vue, Effect, XState) that the server serves |
| `skill/quadratique` | The skill that `quadratique init` writes into a repository; the binary embeds it |
| `install` | The install and uninstall scripts and the systemd user unit |
| `eval` | The evaluation: end-to-end runs and a harness that compares Quadratique with other trackers |

Run every task through moon, for example `moon run quadratique-server:test`:

| Task | What it does |
|---|---|
| `quadratique-server:test`, `quadratique-cli:test` | The Rust tests |
| `quadratique-server:lint`, `quadratique-cli:lint` | Clippy, denying warnings |
| `quadratique-server:fmt-check`, `quadratique-cli:fmt-check` | rustfmt |
| `quadratique-catalog:write` | Renders `catalog/quadratique.pkl` to `catalog/quadratique.acidcat.json`. Run it after each change to the catalog |
| `quadratique-catalog:check` | Fails when the JSON is not what the Pkl renders, or when `acid-cat check --strict` refuses it |
| `quadratique-web:test`, `quadratique-web:e2e` | The web app's headless tests, and its journeys in Chromium against the real binary |
| `quadratique-web:typecheck`, `quadratique-web:lint`, `quadratique-web:format-check` | Static checks of the web app |
| `quadratique-install:test` | Installs on a temporary home with no systemd, and checks the service's cases and the uninstall |
| `quadratique-eval:test` | The evaluation harness's own tests |
| `quadratique-server:package-test` | The packaging preflight's cases, and how `build.rs` embeds the web app in the repository and in a packaged crate (see "The crates") |

### Continuous integration

The workflow [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml) runs on every pull
request and on every push to `main`. On `ubuntu-latest`, it installs the tools that `.prototools`
pins, the JavaScript dependencies from the lockfile and Playwright's Chromium, then runs every
task of the table above in one `moon run`. Nothing is left out: the runner has the commands that
the tests call, and lets an unprivileged process create a user namespace.

### See a change in the web app

The installed binary serves the copy of the web app embedded in it at build time, so it never
shows a change you make. To see one, run a debug build. It is built without embedded assets and
reads the web app from `web/dist` of its source tree at each request.

From the root of the repository:

```sh
moon run quadratique-web:build quadratique-cli:build
```

Then, from `apps/quadratique` (the Rust workspace, the folder with `Cargo.lock`), start that
build's server on a throwaway data folder, so that it neither touches your store nor competes with the installed
server:

```sh
export QUADRATIQUE_HOME="$(mktemp -d)"
target/debug/quadratique ui --open      # starts this build's server and opens the web app
```

`--open` opens the page with `xdg-open`; without it, `ui` only prints the address.

While it runs, `moon run quadratique-web:dev` rebuilds `web/dist` whenever a source changes; reload
the page to see each build. Stop the server with `target/debug/quadratique server stop`, in the same
shell. A binary of any build serves another folder when the variable `QUADRATIQUE_WEB` names it in
the environment of the command that starts the server. Give an absolute path: the server runs in
its data folder and reads a relative path from there, so the page would answer 503.

### Releases

A pull request is squash-merged, and its title, which becomes the commit on `main`, is a
[Conventional Commit](https://www.conventionalcommits.org). Only `feat:` (a minor release) and
`fix:` (a patch release) make a release; `perf:`, `revert:`, `refactor:`, `chore:`, `docs:`,
`style:`, `ci:`, `test:` and `build:` do not. A breaking change (`!` after the type, or a
`BREAKING CHANGE:` footer) makes a release under any of these types: before 1.0.0, a minor one.

[release-please](https://github.com/googleapis/release-please) makes the releases from those
commits. The workflow [`.github/workflows/release.yml`](../../.github/workflows/release.yml)
runs on every push to `main`:

1. release-please opens or updates the release pull request. It sets the new version in
   `Cargo.toml`, `Cargo.lock`, the three `package.json` files, `pyproject.toml` and `uv.lock`
   together, and adds the changes to `CHANGELOG.md`. It writes the pull request again at every
   push (`always-update`), also when the push adds nothing to the changelog, so the pull request
   always contains `main`.
2. When that pull request is merged, release-please creates the tag `v<version>` on the merge
   commit and a draft GitHub release.
3. A job that checks out nothing names the release to finish: the release whose tag is `v` and
   the version that `.release-please-manifest.json` says at the pushed commit, found by its exact
   tag, even when release-please failed after it made the draft. It builds a draft only when the
   tag points at the commit that was released. A published release with its four files needs
   no build; a published release without them fails this job and is not changed. For a draft or
   a complete release, the job also names the commit to publish the crates from: the commit that
   the tag points at, which must be the commit that the release records as its target. A public
   release whose tag was moved fails this job, and nothing is published.
4. One job per platform checks out that commit by its SHA, checks that the tag points at it,
   builds the web app and the release binary with it inside on the oldest hosted Ubuntu (22.04),
   and keeps the archive and its SHA-256 file as workflow artifacts. `install/package.sh` makes
   the archive. The job prints the libraries and the newest glibc that the binary needs. It holds
   no token.
5. A last job, which checks out nothing and runs no project code, checks that the four files are
   there and that each archive matches its SHA-256 file. It works on the release by its ID and
   changes files only while the release is a draft: it removes any file whose name or digest is
   not one of the four, uploads the missing ones, checks the four digests, checks the tag again,
   and only then publishes the release. On a release that is already public it changes nothing:
   it succeeds when the four digests match and fails otherwise.
6. Once the release is public, or at once for a release that was already complete, three jobs
   publish the crates that crates.io lacks, `quadratique-server` then `quadratique`, so that
   crates.io never holds back the release:
   - "crates to publish" reads crates.io's index and names the crates that lack the release's
     version. When both are there, the next two jobs do not run.
   - "build the crates" checks out the tag's commit by its SHA, checks that both crates have the
     release's version, builds the web app, stages it in the server crate and runs
     `preflight.sh` (the steps of the task `quadratique-server:preflight`, which moon does not
     run in a workflow), then runs `cargo package --workspace`, which builds each crate from its
     package, and records the SHA-256 of each package. It runs project code, so it holds no
     token.
   - "publish the crates to crates.io" runs the checkout, rustup and cargo, and no project code:
     no install script, no moon task, no build. It reads the index again, packages the missing
     crates with `cargo package --no-verify` and the staged web app of the build job, and stops
     unless each package has the SHA-256 that the build job recorded. Only then does it get a
     crates.io token through Trusted Publishing (`rust-lang/crates-io-auth-action`), which lasts
     30 minutes and is revoked when the job ends, and runs `cargo publish --no-verify`. cargo
     uploads the server first and waits up to 60 seconds for the index to list it before it
     uploads the command line. Last, the index must record the recorded SHA-256 for both crates.

   A crate version that the index already lists, with the recorded SHA-256, is skipped. When the
   index already lists both crates, nothing is built and nothing compares their bytes. No job
   holds an App token and a crates.io token together, and no secret is stored for crates.io.

The workflow writes with tokens of a GitHub App, not with `GITHUB_TOKEN`: GitHub starts no
workflow for a tag that `GITHUB_TOKEN` pushes, and holds the checks of a pull request that it
opens. Each token is for this repository only: release-please's can write contents and pull
requests; the two other jobs' tokens can write contents only (a token needs write access to see a
draft release). The workflow reads the App's client ID from the repository variable
`RELEASE_APP_CLIENT_ID` and its private key from the secret `RELEASE_APP_PRIVATE_KEY`.

crates.io accepts the publish job's token because each crate has a Trusted Publishing
configuration on crates.io: repository owner `Hebilicious`, repository name `quadratique`,
workflow file `release.yml`, environment `crates-io`. That job runs in the repository's
environment `crates-io`, which accepts only runs on `main` and asks nobody for anything: no
reviewer, no wait. It exists because crates.io does not check the branch and this workflow also
runs on pull requests; GitHub refuses the environment to a pull request, so a pull request that
edits the workflow gets no crates.io token.

A pull request that changes the release workflow, its configuration, `.github/release-tests/` or
`install/` builds the archives too, as workflow artifacts, and releases nothing. It also runs
`node .github/release-tests/release-please.cjs`, which runs release-please as the pinned action
bundles it on this configuration and checks that it opens `chore(release): release <version>` and
that the merged pull request yields the draft release `v<version>`, and
`bash .github/release-tests/steps.sh`, which runs the jobs' scripts against a fake `gh`, a fake
crates.io index and a fake `cargo`. Both run locally too, with no token.

#### When a release stops half-way

The release stays a draft until its publish job has attached and checked the four files, so a
failure never leaves a public release without its archives. The crates are published after the
release, so a crates.io failure never holds back the release's files.

- **A job failed** (a flaky runner, a network error, release-please after it made the draft): in
  the workflow run, use "Re-run failed jobs". The builds run again on the same commit, replace
  their artifacts, and the publish job replaces only the files that are wrong.
- **A crates job failed:** use "Re-run failed jobs". The publish job reads crates.io's index
  again and publishes only the crates that it does not list, so a run that published
  `quadratique-server` and then failed publishes only `quadratique`. Before you run it again,
  look at `https://crates.io/crates/quadratique-server/<version>` and
  `https://crates.io/crates/quadratique/<version>`: a version that crates.io shows but the job's
  log says is missing from the index is still on its way to the index; wait a minute. If the job
  fails at the token step, check the crate's Trusted Publishing configuration on crates.io
  (Settings, Trusted Publishing): it must name `Hebilicious`, `quadratique`, `release.yml` and
  `crates-io`. Also check that the repository's environment `crates-io` still allows `main`. If
  it stops because a package's SHA-256 is not the one the build job recorded, nothing was
  published: run all the jobs of the workflow again, not only the failed one. The GitHub release
  is already public with its files at this point; the crates jobs never change it.
- **The run is too old to re-run:** push any commit to `main` (a `docs:` or `chore:` one releases
  nothing). This works only while `.release-please-manifest.json` on `main` still names the
  stopped release: the workflow finishes the draft of the manifest's version, and publishes the
  crates that crates.io lacks at that version. Once a later release pull request is merged, the
  manifest names the later version, and a push works on that one only.
- **An older release stopped after the manifest moved on:** for its GitHub release, use the "By
  hand" steps below. For its crates, there is no way today: crates.io gives a token only to this
  workflow on `main`, and the workflow publishes only the manifest's version. Its crates stay
  unpublished unless the owner decides otherwise; the next release's crates are published as
  usual.
- **By hand**, if the workflow cannot run, in this order:
  1. On each platform: `git checkout refs/tags/v<version>`, `moon run quadratique-web:build`,
     then `sh apps/quadratique/install/build.sh apps/quadratique/web/dist apps/quadratique/target`
     and `sh apps/quadratique/install/package.sh <binary> <version> <target triple> <folder>`.
  2. Put the four files in one folder and check them there: `sha256sum --check --strict *.sha256`.
  3. Check that the release is still a draft:
     `gh release view v<version> --json isDraft --jq .isDraft` must print `true`. If it prints
     `false`, stop: do not change a public release's files.
  4. `gh release upload v<version> <the four files> --clobber`.
  5. Check the attached digests against the local files:
     `gh release view v<version> --json assets --jq '.assets[] | "\(.name) \(.digest)"'` must
     list exactly the four files, each with `sha256:` and the hash that `sha256sum` prints for it.
  6. Only then: `gh release edit v<version> --draft=false`.

### The crates

The command line and the server are the crates `quadratique` and `quadratique-server`. Both
inherit `publish = true` from `[workspace.package]`. Each crate
carries the files it reads at build time: the catalog JSON, the skill and the license are links in
the crate to the files of the repository, and `cargo package` copies their content.

The web app is embedded in every binary that people install, and in none that you build in the
repository without asking:

- A packaged crate, as crates.io serves it, embeds the copy of the built web app it carries in
  `server/web-dist` (ignored by git), which `moon run quadratique-server:stage-web` writes. Its
  `build.rs` fails, and says to stage it, when that copy is missing, and ignores
  `QUADRATIQUE_WEB_DIST`. It tells a package from the repository by `server/moon.yml`, which the
  package leaves out.
- In the repository, the release build (`install/build.sh`) asks for it with the feature
  `embedded-web`, which embeds the folder that `QUADRATIQUE_WEB_DIST` names, else `web/dist`.
- Any other build in the repository embeds nothing and reads `web/dist` at each request, as "See a
  change in the web app" describes.

`moon run quadratique-server:preflight` first builds the web app and stages it (it depends on
`stage-web`), then fails when a crate lacks the staged web app (or it is not the current build of
`web/dist`), the catalog JSON, a skill file or the licence. It also catches a checkout without
symbolic links, where git writes each link as a plain file holding a path. It is a task for a
person's machine: `stage-web` and `preflight` have `runInCI: false`, so in a workflow moon answers
"No tasks found" for them, and the Release workflow runs their steps itself.
`moon run quadratique-server:package-test` checks the preflight's cases and each case of
`build.rs`, on a copy of the crates that it stages and packages itself: it leaves this checkout's
`server/web-dist` as it was.

## Releasing

What a maintainer does for a release, and what happens on its own:

1. Merge pull requests into `main` with Conventional Commit titles (see "Releases"). When one of
   them releases, release-please opens or updates the release pull request on its own.
2. Review and merge the release pull request. The tag, the draft release, the archives and the
   publication of the release follow on their own; check that the run of the workflow
   `Release` is green and that the release lists four files.

The same run publishes both crates to crates.io (step 6 of "Releases"): no token from a person
is needed. Check that its job "publish the crates to crates.io" is green and that crates.io shows
the new version of `quadratique-server` and `quadratique`. Release 0.2.0 is the first release
published this way; 0.1.0 was published by hand.

## License

Quadratique is released under the MIT License. See [`LICENSE`](../../LICENSE).
