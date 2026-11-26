# 0004. A socket for commands, a token for the browser, and no service

Date: 2026-11-25 (revised after the design review of the same day, and on 2026-11-26 with the owner's answers to
its open questions and three corrections)

Status: proposed. Supersedes [0003](0003-the-caller-check-on-macos-and-windows.md), and replaces
the Linux kernel check that 0003 kept.

## Context

The server has one TCP listener on `127.0.0.1`, which the command line, `quadratique mcp` and the
browser all use. On Linux it asks the kernel which account owns each connection
(`server/src/peer.rs`). Elsewhere it refuses to start. `install.sh` writes a systemd user unit.
`upgrade` restarts that unit, or signals the server's process number through a pidfd.

| # | Weakness that reviews found |
| --- | --- |
| W1 | While the server is down, another account can bind its port. A bookmark then reaches that account, which can read the origin's `localStorage`. |
| W2 | The command line trusts whoever answers on the port, and acts on the process number in the answer. |
| W3 | The kernel lookup identifies a connection by addresses and ports, which can be reused (QUA-46). |
| W4 | `upgrade` must find and signal the right process, and must know systemd. |

The owner's decisions (2026-11-25, QUA-48) are fixed:

1. The command line and the MCP server reach the server through a Unix socket in the private data
   folder. File permissions decide who connects, in both directions. They use no TCP port and no
   kernel lookup.
2. The Linux kernel check goes too. Every system uses one mechanism.
3. Only the web app uses TCP. It requires a token that `quadratique ui` gives to the page, and the
   page saves it in the browser.
4. The service leaves the product. Installing means putting one binary on the PATH. The documentation
   explains how to keep the server running after a reboot.
5. Stop and take-over are requests on the socket.
6. Windows waits for QUA-47. This record names the Windows equivalent and builds nothing for it.

The owner's answers to the open questions (2026-11-26) are fixed too. They go against this record's
first recommendation, which was a bearer per server start and a kept port as an opt-in:

7. "we want convenience and good enough security by default. the ux has to be really
   straightforward for humans and not cumbersome". So, by default, the server keeps its port across
   restarts, and the bearer is one per data folder: made once, kept in the private data folder, and
   valid across restarts and upgrades until a command rotates it. A person runs `quadratique ui`
   once per browser, with nothing to type or copy; after that, bookmarks and open tabs just work. A
   browser that is not paired sees one plain sentence with the one command to run. Nothing about
   tokens, codes or ports is asked of the person.
8. The stricter behaviour, a fresh address and a new bearer at each server start, is one opt-in
   setting, for a machine whose other accounts are not trusted (§4, "Port policy").
9. AcidCat gets the native Unix-socket mode: the owner opened
   [acidcat#235](https://github.com/Hebilicious/acidcat/pull/235), which adds
   `HttpClient::unix_socket(path)` to its Rust client.

**Measured facts.** These come from a throwaway probe of the first round (Linux 7.2, not committed),
earlier measurements, and the reviewer.

| Fact | Result |
| --- | --- |
| `connect` with no socket file / to a stale socket file / a second `bind` | `ENOENT` / `ECONNREFUSED`, and the file stays / `EADDRINUSE` |
| Mode of a new socket under umask 022 | `0755` |
| Path of 107 bytes, then 108 bytes (Python's `socket`, not the Rust API) | bound, then refused |
| `nobody` connects. Folder `0700`, socket `0777` / folder `0755`, socket `0600` / both loose | `EACCES` / `EACCES` / connected |
| Another account that can write a non-sticky **parent** renames a `0700` folder without entering it | confirmed by the reviewer on Linux |
| A plain Claude Code shell, then `codex sandbox` (default policy), connect to a `0600` socket in the data folder | connects, then fails (loopback TCP fails there too) |
| The same, measured again in QUA-53 (2026-11-26, Claude Code 2.1.332, codex-cli 0.172.0); and `codex sandbox` with `network_access = true` | connects; `socket(AF_UNIX)` gives `EPERM`; the socket and loopback TCP both connect. In both Codex cases the home policy refuses first: the sandbox's user namespace leaves root unmapped, so `/home` shows as uid 65534 (QUA-84) |
| Both `codex sandbox` policies again, with the build of QUA-84's first part, `quadratique issue list` | the command tells the overflow uid from an account (no line of `/proc/self/uid_map` maps that uid), says the sandbox hides who owns `/home`, and answers `unavailable` with its MCP tool, exit 4; the home policy still refuses the folder, and under the default policy `socket(AF_UNIX)` succeeds and `connect` gives `EPERM` |

## What AcidCat gives

Sources: acid-cat-core 0.6.0, in `~/.cargo/registry/src/index.crates.io-*/acid-cat-core-0.6.0/src/`.

| Side | Finding | Consequence |
| --- | --- | --- |
| Server | `remote::serve` binds TCP (`remote.rs:323-324`). But `Handler` and `Routes` (`remote.rs:33`, `:320`) take `http::Request` and do not depend on a transport. Quadratique already runs its own accept loop (`server/src/serve.rs`). | No change in AcidCat. `serve.rs` gets a `UnixListener` loop. |
| Client | `HttpClient::new` accepts only an `http` or `https` URL with a host (`http_client.rs:52-69`). `send` builds the `Host` header from the URL and turns every dial error into one "does not answer" (`http_client.rs:186-205`). `client::connect` dials `TcpStream` (`client.rs:36`) and is `pub(crate)`. `CliOptions::remote` accepts only `HttpClient` (`cli.rs:295`, `:336`). AcidCat `main` (0.8.0) is the same. | The command line needs a change in AcidCat: a socket connector, a fixed HTTP authority (for example `http://quadratique.invalid`), Unix dialing that never falls back to TCP, and its own dial errors (QUA-51). Quadratique still does its own discovery and diagnostics before it builds the client (§2). |
| Web client | `@acid-cat/core-effect` 0.13.1 uses `fetch` and takes `headers` (`dist/internal/httpClient.d.ts`). | The live channel can carry `Authorization`. |

| Option for the client | Cost |
| --- | --- |
| **A. AcidCat: `HttpClient` gets a Unix-socket mode** (recommended, QUA-51) | The connector, the authority, the URL check and the error kinds, with tests of a generated call, capability discovery and a streaming `watch`. Then Quadratique moves from `=0.6.0` to that release: a narrow 0.6.x release if feasible, otherwise 0.8, whose size is estimated separately (QUA-52). |
| B. Quadratique's own client below the generated command line | A second copy of `invoke`, `capabilities` and the SSE parser (`http_client.rs` is 497 lines), plus the MCP upstream. It would drift from AcidCat. |
| C. A loopback relay | Rejected: it brings TCP back. |

## 1. The data folder and the socket

**One policy, shared.** One function in `quadratique-server` validates and creates the home. The
server and the command line both call it, before either opens a lock, touches the store or spawns a
process. Today the command line creates the folder and opens lock files (`cli/src/background.rs:179-186`)
before the server applies `0700`, and the server follows the path when it does (`server/src/server.rs:380`,
`private`). Both behaviours go. The rule reuses the walk in `cli/src/upgrade/folder.rs:23-46`.
The model is the same as that file's: the policy defends against other accounts, not against
processes of the same account.

| Step | Rule | Refused with |
| --- | --- | --- |
| 1. Resolve | Every component of the given path is examined with `lstat`. A symbolic link is allowed only when the link and the folder that holds it are owned by this account or root, and the folder passes step 2. System aliases such as macOS's `/var`, `/tmp` and `/etc` (root links into `/private`) pass this way. The checks below run on the resolved path, and all later work uses it. | "`<link>` is a link that another account could change" |
| 2. Ancestors | Each ancestor is owned by this account or by root (uid 0). Group or other write is allowed only on a sticky ancestor (`/tmp`). This is `folder.rs`'s rule. | "another account can replace `<ancestor>`" |
| 3. Effective access | Mode bits are not enough. The leaf and every non-sticky ancestor that this account owns must carry no extended ACL entry that grants write (or, for the leaf, any access) to another user or group. On Linux, read `system.posix_acl_access` and `system.posix_acl_default` with `rustix::fs::getxattr`. On macOS, the safe call that reads an ACL is still to be found: until QUA-50 finds one and tests it, an ACL on macOS counts as not verified, and the home is refused if one exists. A group-shared home is refused: the leaf's mode must be exactly `0700`. | "`<path>` gives access to another account" |
| 4. Leaf | If the leaf is missing, it is created private from the start: `DirBuilder` with mode `0700`, and the same for any missing ancestor, which is then checked again. If the leaf exists with another mode, the home is refused with `chmod 700 <path>`. The policy never runs `chmod` through a path. | "`<path>` is not private" |
| 5. Socket path | The raw byte length of `<leaf>/server.sock` must be at most 107 on Linux and 103 on macOS (`sun_path`, one byte for NUL). Tests exercise the Rust and tokio API at the limit and one byte over it, on both systems. | the path, its length, the limit, and "set `QUADRATIQUE_HOME` to a shorter folder" |

**The socket entry.** Only the holder of `server.lock`, under the verified lock, acts on the name
`server.sock`:

| What `lstat` finds | The server |
| --- | --- |
| nothing | binds |
| a socket owned by this account | removes it, then binds |
| anything else: a regular file, a link, a folder, or a socket of another uid | refuses to start, and names it |

After `bind`, the server sets `0600` and checks with `lstat` that the entry is a socket of its uid
with mode `0600`. Only then does it accept connections and count as ready. It never changes the
process-wide umask: the server has many threads. A connection queued before that check waits in
the backlog, and the folder already keeps out other accounts. A client checks the entry the same
way before it trusts an answer. A socket with any other mode means "not ready yet" within the
deadline.

**The file-system boundary.**

| Case | Decision |
| --- | --- |
| Namespace | Pathname sockets only, never the Linux abstract namespace, which has no permissions. |
| Supported storage | Trusted local storage on one host. On NFS, the scope of a lock depends on mount options, and a socket file carries no connection between hosts: unsupported and untested. |
| Container | It needs a view of the same socket (a bind mount) and a uid mapping that grants access. Documented, not tested. |
| Sandbox | A sandbox can block the socket or hide it. Only **confirmed absence** (the leaf validated and readable, and `lstat` gives `ENOENT`) or **refusal** (`ECONNREFUSED` on a valid socket entry) allow a start. Policy denial (`EACCES` or `EPERM` on the folder, the lock or `connect`) gives `Unreachable`. A path error (`ENOTDIR`, `ELOOP`, `ENAMETOOLONG`) is reported as such. A transient failure (`EINTR`, `EAGAIN`) is tried again within the deadline. |
| File system without sockets | `bind` fails, and the server refuses to start. No TCP fallback. |
| Two servers | No change: `server.lock` (`flock`) gives one owner per home. |

**Peer credentials.** tokio gives a safe `UnixStream::peer_cred()` on Linux and macOS (tokio
1.54.1, `src/net/unix/stream.rs:981`, `src/net/unix/ucred.rs:37-60`). It would add a second fence
that holds if the folder policy were wrong. It is not taken, because decision 1 makes file
permissions the rule, and a second rule doubles what each platform must test.

## 2. States and transitions

Each server start has a **generation**: 128 random bits, reported by `GET /server`. The answer also
carries a **control protocol version** (`control: 1`), apart from the catalog interface hash. The
control protocol covers `GET /server` and `POST /server/stop`. Every build keeps answering every
control version it has ever served, so a command line of any build can stop any newer server that
still lists its version, and an older command line can stop a newer server.

**Handoff rule.** Every start holds `start.lock`: an on-demand start (the spawner holds it for its
child until the child is ready, or until the child exits), `quadratique serve`, and `serve
--take-over`. Stop also holds it while it waits. So no successor can take `server.lock` between a
stop and the moment its stopper sees the lock free. The generation tells a stopped server from its
successor in what the command line reports.

**Stop acknowledgement.** `POST /server/stop {generation}` answers `409` if the generation is not
this server's. Otherwise the server closes its listeners (no new connections), writes `202
{generation}`, and starts its shutdown only after that response has been flushed. Open connections
get a bounded drain, then the store closes and `server.lock` is freed.

One deadline per operation: `START_LIMIT` (30 s) for ensure and take-over, and `STOP_LIMIT` (30 s)
for stop. `PROBE_LIMIT` (5 s) bounds one identity request.

| Observation | `status` | ensure (every command) | `server stop` | `serve --take-over` |
| --- | --- | --- | --- | --- |
| No socket file, `server.lock` free | `running: false` | take `start.lock`, **observe again**, spawn, wait for ready | `NotRunning` | take `start.lock`, observe again, serve |
| Stale socket file (`ECONNREFUSED`), lock free | `running: false` | same as above; the new server replaces the file | `NotRunning` | same as above |
| Socket refuses or is missing, lock held, no `server.json` (a server that holds the lock and has not bound yet, or one that is stopping) | `starting: true` | wait within the one deadline for ready or for the lock to be freed. Freed: back to row 1 under `start.lock`. Deadline: `NotReady` with the log | wait within the deadline; ready: stop it; freed: `NotRunning` | as for stop, then serve |
| Same, and a `server.json` names a TCP address | `legacy: true` and what to stop | `Legacy`: no spawn; the message names `server stop` | the legacy stop (§3), or instructions | the legacy stop, then serve |
| Connects, no full answer within `PROBE_LIMIT` | `stalled` | `Stalled`: never spawn | `Stalled`: the person ends it | refuse |
| Answers a malformed identity | `malformed` | `Malformed`: never spawn | refuse | refuse |
| Names another database | `otherDatabase` | `OtherDatabase`: never spawn | refuse | refuse |
| Same control version, other interface (another build) | `sameInterface: false` | `OtherInterface`: run `server stop` | stop the generation | stop, then serve |
| A control version this build does not know (a newer server) | `newer` | `Newer`: upgrade this command line | stop if the server lists this build's version, otherwise refuse | refuse |
| Answers, same database, same interface | `running: true` | use it | stop the generation | stop the generation, wait for release, serve |
| Two starters at once | — | the second waits on `start.lock`, then observes again and finds the first one's server | — | — |
| `EACCES`/`EPERM`, path error | `reachable: false` and the reason | `Unreachable` (MCP advice as today); never spawn | `UnreachableToStop` | refuse |

**As built in QUA-53 (2026-11-26).** Every command, `quadratique mcp`, `init` and `upgrade --skills`
build their client in one place (`caller::client`) from the endpoint that `ensure` or `observe`
gives: on Unix the home's socket, with acid-cat-core 0.9.0's `HttpClient::unix_socket` and the base
`http://quadratique.invalid/cli`. That client dials only the socket, never TCP, and streams `watch`
through the same connection. It reports every failure to connect or to get an answer as one call
failure, "The server at http://quadratique.invalid/cli does not answer.", with no typed error. So the
rows above stay Quadratique's: a command calls only after `observe` found the row "answers, same
database, same interface", and that failure is left for a server that stops between the observation
and the call. `init`, `upgrade --skills` and `quadratique mcp` send their calls themselves and say it
in Quadratique's words: the server stopped after it was found on `<home>/server.sock`, and what to
do; they recognise the failure by the client's own words for its base URL. The generated commands
still print AcidCat's words, because AcidCat's command line sends their calls and offers no hook to
reword a failure. A data folder that this process may not enter (`EACCES` from the home policy:
another account, or a sandbox that hides it) is `Unreachable` for every caller, exit 4, and
`status` answers `reachable: false`; `serve`, the server itself, keeps its internal failure.

The socket serves the `cli` routes beside the control requests. Each connection is either receiving
a request, which must arrive whole (head and body) within the limit (5 s, `Waits::socket_request`),
or answering one, which has no limit; the end of an answer starts the deadline of the next request.
File permissions alone admit a caller on the socket: `root` passes them. Windows still finds its
server over TCP (QUA-61).

**A hung server** (stalled, or one that holds the lock past the deadline) gets a bounded outcome,
never a signal. The message names the home, the log, and the unverified pid from the last identity
or `server.json` (for diagnosis only). It says how to recover: Ctrl-C in its terminal, the
person's own supervisor, or ending the process after checking it. A command line may end a child
**it spawned itself** through its own `Child` handle, because that handle cannot point at a reused
pid.

`server.json` stays written until the cutover (QUA-58) and is then dropped. After the cutover it is
only a hint that a 0.1 or 0.2 server may be running. A stale or malformed `server.json` changes
nothing in the table except the legacy row.

## 3. Lifecycle and upgrade

| Question | Decision |
| --- | --- |
| Start | On demand, by any command, as today. `quadratique serve` stays as the way to run the server in the foreground (a terminal, or a supervisor the person sets up). |
| Stop when idle | No. An idle stop would cost the web app its live updates (and, in strict mode, its bearer), and the git watcher its looks. One idle process costs little. |
| Git watcher | No change: it looks every 2 s, and a read asks for a look. After a reboot, the next command's server catches up. |
| Signals | The server still stops on SIGTERM and SIGINT. The command line sends none, except the proven legacy stop below. |

**`upgrade`.** It keeps every check it has today: refusal inside a git working tree, `folder::check`,
the schema check (`store_opens`), the downgrade refusal, and the install lock that records the
replaced file for an interrupted upgrade (`cli/src/upgrade/mod.rs:166-200`, `upgrade/folder.rs:57-95`).
What changes is how it identifies the server:

| Step | Rule |
| --- | --- |
| Before the replacement | It reads `GET /server` on the socket: the generation G and the **executable identity** E. E is the device, inode and path of the running file, which the server captures at its start from `std::env::current_exe` and its metadata. The answer comes over the verified socket, so it is as trusted as the account. |
| Decide | E equals the file being replaced: stop G after the rename. E is another file (another install): leave it running, and say so. No answer: nothing to stop. |
| Stop | `POST /server/stop {generation: G}`. A `409` means another generation now serves: nothing more is stopped. |
| Interrupted upgrade | A later run reads the replaced file recorded under the install lock, and stops the server whose E is that file, as `upgrade/server.rs:168-190` does today with `/proc`. |
| Started with `serve` | The identity says `started: serve`. The upgrade stops it and says: "restart it the way you started it (your supervisor, your terminal)". |

**Legacy TCP server (0.1, 0.2).** Its `GET /server` answer (pid, database) is untrusted: any account
could answer on that port. A pidfd pins a process only after it was selected. So an automatic stop
needs **independent evidence**, on Linux only. (Correction of 2026-11-26: release 0.1 built from
source also ran on macOS, where it admitted every caller, because only Linux had the kernel check.
No release archive or installer served there; a 0.1 server on macOS gets the manual instructions.)

1. `pidfd_open(pid)`;
2. through `/proc/<pid>`, read while the pidfd pins the process: `exe` is the old installed binary
   (`${XDG_BIN_HOME:-$HOME/.local/bin}/quadratique`, where release 0.1's `install.sh` put it).
   (Correction of 2026-11-26: an earlier revision also counted the replaced file recorded by the
   install lock. That can never match a 0.1 server, because release 0.1 has no `upgrade`, and pull
   request 38 does not use it.)
3. `fd/` holds a descriptor to this home's `server.lock` (same device and inode);
4. `pidfd_send_signal(SIGTERM)`, then wait for `server.lock` to be freed.

Without all of it (another system, no pidfds, a binary other than the installed one), the
command line stops nothing and tells the person what to stop: `systemctl --user stop
quadratique.service` if the unit runs it, otherwise the process (pid given as unverified, with
`ps -p <pid> -o args`). Detection (lock held, no socket, `server.json` present) stays permanently
because it is cheap. The automatic stop is deleted once the documented manual recovery covers
every case, and no earlier than two minor releases after the cutover (QUA-55).

## 4. The web app on TCP

**Routes on TCP.** Every request needs the exact numeric `Host: 127.0.0.1:<port>`. Anything else
gets `403`, which closes the `localhost` → `[::1]` squat (`web.rs` `Origin::hosts` accepts
`localhost` today).

| Path | Methods | Auth | Answer |
| --- | --- | --- | --- |
| `/`, `/now`, `/all[/board\|/history]`, `/projects/<key>[/board\|/history]`, `/issues/<id>` (the deep links of `web/src/route.ts:36-38`, `:177-192`) | `GET`, `HEAD` | none | the app shell |
| files that exist in the built tree (`/assets/*`, `index.html`, …) | `GET`, `HEAD` | none | the file |
| `/cli`, `/server`, and everything below them | all | — | `404`, matched **before** the single-page fallback (today the fallback answers `200` with HTML for unknown paths without an extension: `web.rs:246-255`) |
| any other path | all | — | `404` |
| `/session` | `POST` | the one-time code | the bearer |
| `/web/*`, `/webmcp/*`, including the event stream | as the binding | `Authorization: Bearer` | `401` without a valid bearer |

**Pairing contract.**

| Part | Rule |
| --- | --- |
| Order | On `ui`'s request (over the socket), the server first binds the TCP listener and installs the guarded routes, then makes the code. It returns the real address and the code together, so `ui` never hands a code to a listener that is not its own server. |
| Code | 32 random bytes, single use, valid 60 s (an injected clock in tests). At most 16 codes are pending; beyond that, the request is refused and `ui` says so. |
| `POST /session` | Requires `Origin` exactly equal to `http://127.0.0.1:<port>`. Sends no CORS headers at all. The body is at most 1 KiB of JSON. Redemption is atomic: the code is checked and deleted in one step under one lock. The answer carries `Cache-Control: no-store`. |
| Bearer | 32 random bytes, compared in constant time. By default, one per data folder (owner, 2026-11-26): made the first time it is needed, kept in `web-token` in the private data folder created exclusively with mode `0600` under the home that `home::prepare` verified, so it survives restarts and upgrades. `quadratique server sign-out` rotates it, which signs every browser out. In strict mode it is made per server start and kept in memory only. It never appears in an answer, a log, an error, a process list or a URL, except in the answer of `POST /session` to the page. `web` and `webmcp` keep their audiences (the human, and the agent `browser`) but share it. |
| Bootstrap file | The server creates `<home>/open-<random>.html` with `create_new` and mode `0600`. It holds a meta refresh to `http://127.0.0.1:<port>/#pair=<code>` and never lies in the static tree: the server refuses to start if `QUADRATIQUE_WEB` or the built tree is inside the data folder, or the reverse. `ui --open` opens the file **in the caller's desktop session** (the server, which an agent or a supervisor may have started, opens nothing). The server deletes the file when the code is used, when it expires, and at its next start (files left after a crash). Process arguments show only the path. |
| Without `--open` | `ui` prints the link with the code, for confined browsers and remote terminals. The 60 s expiry bounds a code left in a terminal log, in history or in a restored tab. A used code is dead. |
| Page start | Before the app mounts, the first module (`web/src/main.ts`) reads `#pair`, clears it with `history.replaceState`, and redeems it. Subscriptions and the WebMCP registration start only after pairing has succeeded. |
| Storage | The bearer goes in `localStorage` at the origin (the owner's "save the token"). By default it keeps working across restarts; in strict mode, for that server start. |
| `401` and stale credentials | On a `401` from any target, the page drops the stored bearer, empties its per-target client cache (`web/src/core/server.ts:12`), ends its subscriptions, unregisters its WebMCP tools, and shows "Run `quadratique ui --open`". Another tab that pairs fires a `storage` event, and open tabs rebuild their clients with the new bearer. |

**Port policy.** By default the server **keeps its port** (owner, 2026-11-26). It asks for the
port it had last, which it keeps in `server.port` in the data folder, and takes one the system
chooses, which it then keeps, when another program holds it. `serve --port <port>` (0 included)
reads and keeps nothing. The **strict setting** (`quadratique server strict on`) gives a **freshly
bound origin** instead: the server binds a port its home has never served. It lists the ports it
served in the data folder (the last 4096), and binds again, up to 8 times, if the system gives one
from the list. Turning strict on adds the kept port to that list. Turning strict on or off starts no server
and applies from the next start; the first start after `strict off` takes a new address and keeps it. A start
reads the setting once, under the home's lock. A normal start that read it just before `strict on` may write
`server.port` again; every strict start therefore adds a kept port to the served ports and removes `server.port`
first. `addressKept` is true only when the next start would ask for the address in use: the server kept that port,
strict is off now, and `server.port` names that port now. The same setting makes the bearer
per server start: one setting, for a machine whose other accounts are not trusted.

**Threat analysis.** B is another account on the same machine. A machine with a single account has
no B.

| What B can do | Default (kept port, lasting bearer) | Strict (fresh origin, per-start bearer) |
| --- | --- | --- |
| Bind the port while the server is down, and serve a page to a bookmark | possible on **the** port that every bookmark and every future pairing uses | possible only on an old port that no future server will use |
| Read the stored bearer | **a bearer that still works** once the server runs again, until it is rotated | a bearer that no longer works |
| Read or change other data kept in that origin's browser storage | none: saved views live in the server (QUA-67), and the bearer is the only thing the page stores | the same |
| Leave a **service worker** that controls later navigations to that origin, even after the real server holds the port again, and intercept the next pairing and the live bearer | **yes**. Neither a health check nor a `Host` guard can tell what code the browser runs. | no future pairing uses that origin |
| Run a script in a paired origin | (any such script can read the live bearer, as any same-origin script can) | the same |

A non-extractable WebCrypto key does **not** make a pairing that survives restarts safe on its own:
code in the same origin, such as B's service worker, can ask the browser to use the key. The record
no longer offers it as a fix.

In plain words, the residual risk of the default is this: on a machine with other accounts, one of
them can take the port while your server is down and serve a page at that address. If you open a
tab or a bookmark of Quadratique then, that page can read what the browser saved for the address,
the token included. With the token, that account can read and change your issues whenever your
server runs, until you rotate the token. Its page can also leave a service worker that keeps control
of the Quadratique tab in that browser. The fix is to rotate the token, clear the site data for
that address in the browser, and turn strict on. The strict mode exists for that machine. No other
machinery is added against this risk.

Decision (owner, 2026-11-26): the kept port and the lasting bearer by default; strict as the opt-in.

**As built in QUA-57 (2026-11-26), before the cutover.**

| Point | What was built |
| --- | --- |
| Token file and rotation | `<home>/web-token`, base64url, written as every file of the home is (`home::write_atomically`: a partial file unique to the write, made with `create_new` and mode `0600`, then renamed). A missing or damaged file is replaced at start. `quadratique server sign-out` (`POST /server/sign-out` on the socket) rotates it and drops the pending codes; the file and the memory change under one lock. A strict start removes the file, and so does `strict off`. `server strict on` and `off` start no server and apply from the next start; the first start after `strict off` makes a new folder token and takes a new address, so each browser pairs once more. |
| Pairing request | `POST /server/pair {"open": bool}` on the socket answers `{url, link, file}`. Older builds answer `404`/`405`, and `ui` tells the person to stop that server. |
| Before the token is required | A request with no bearer is served (the kernel check still guards TCP); a request with a bearer that is not the current one gets `401` already, with `WWW-Authenticate: Bearer` and a call failure that names `quadratique ui --open`. So a page with a stale token after a sign-out or a strict restart meets the real `401` path today. QUA-58 sets `pairing::REQUIRED` to `Required`; a unit test of the guard proves the `401` for a missing bearer in that mode. |
| `/cli` and `/server` on TCP | They answer on TCP without the bearer, guarded only by the kernel check. QUA-53 moves their callers to the socket and leaves the routes on TCP; QUA-58 removes them from TCP, in the same step that requires the bearer. A server test (`web::the_bearer_is_required_only_once_no_command_line_route_answers_on_tcp`) fails if `pairing::REQUIRED` becomes `Required` while either still answers on TCP. The shell fallback never answers them: only the deep links of the route table get the shell, and every other path gets `404`. |
| Open live channels | A sign-out refuses every new request and ends every event stream admitted with the earlier token (QUA-79). Each admitted stream holds the cancellation token of the bearer it was admitted with; `Pairing::rotate` cancels it under the same lock that changes the file and the memory, so once `sign-out` answers, no stream of the earlier token produces another event. An event already handed to the connection before then may still reach the page. The page's stream reconnects, gets `401`, and the page shows the sentence. A stream opened without a token, served until QUA-58, is bound to no token, and a sign-out leaves it open; once the token is required, no such stream exists. A strict restart ends every stream with the server. |
| A link opened in a tab of the same address | The browser changes only the fragment and loads nothing. The page reads `#pair=` on `hashchange` too, and clears it. A paired tab exchanges the code while the app keeps running (`paired.exchanging`), so nothing typed is lost; a refused code, a fake one included, changes nothing. A new token from the code is saved and goes to the running app, whose clients read the token at each request: the server holds one token, so the old one is stale, and nothing restarts. A `401` or a cleared token during the exchange drops the old token and waits for it: its token keeps the app running, its refusal leaves the tab unpaired. A later code replaces one that is being redeemed. |

**As built in QUA-58 (2026-11-26), the cutover's first step.**

| Point | What was built |
| --- | --- |
| Routes on TCP | Only the web app's: the guarded `/web` and `/webmcp`, `/session`, and the files and deep links. `/cli`, `/server` and every path below them answer `404` for every method, before the single-page fallback; any other path that is neither a file nor a deep link answers `404` for every method, and a deep link or a file answers `405` to a method other than `GET` and `HEAD`. The exact numeric `Host` is checked first on every route (`403`). |
| The bearer | Required on `/web`, `/webmcp` and the event stream: a request without it gets the same `401` as one with another (`Requirement` and `REQUIRED` are gone). No stream without a token exists, so a sign-out ends every stream. |
| When the listener opens | Not at every start. The bearer is made at the first pairing (the record's "made the first time it is needed"): by default it is then written to `web-token`, in strict mode kept in memory. A start opens the TCP listener only when the home already has a bearer, that is, by default, after any earlier pairing, so bookmarks and open tabs work after a restart with nothing to run. Otherwise the first `POST /server/pair` opens it, then makes the code. A server that only agents use listens on no TCP port. The port policy is unchanged and applies when the listener opens; the strict setting is still read once per start. A sign-out with no bearer replaces nothing. |
| `server.json` | No longer written. The server's own fast refusal of a second server (`Owned`) asks the socket for the identity, and still asks the TCP address of a `server.json` that a server of releases 0.1 to 0.3 wrote. |
| The identity | It gains `web` (`http://127.0.0.1:<port>/`) and `addressKept` once the listener is open. `url` stays, always `http://127.0.0.1:0/cli`: see the correction in §6. `server status` loses `url` and `runtime`, and gives `web` only while the web app listens. |
| Windows | Its command line still looks for a server through `server.json` and TCP (`background/over_tcp.rs`); no server writes that file, and none starts on Windows. QUA-59 deletes that path with the signal paths it uses. |
| A Windows browser on a server in WSL | Not measured: this machine has no Windows. The bearer no longer depends on the kernel, so the WSL relay is no obstacle in principle. |

## 5. Remote access (PRD §6, M7)

The plan does not change, and one rule becomes clearer: "on the machine that runs the server" means
"over the socket". `token.*`, `session.*` and `issue.advance` stay on the `cli` target, which only
the socket serves. The tailnet board and MCP on the internet keep their own backends and
credentials. The local bearer is not valid there.

## 6. Deleted, kept, and direct upgrades

| Deleted (in the cutover) | Where |
| --- | --- |
| The kernel lookup, the admission, the refusal drain, `CallersUnknown`, the refusal to start on macOS | `server/src/peer.rs`, `server.rs` `callers`, `serve.rs` |
| Signals and pids in the command line, except the legacy module | `cli/src/process.rs`, `background.rs`, `upgrade/server.rs` |
| The unit written by the installer, `uninstall.sh`, the fake `systemctl`, the `systemctl` code in `upgrade` | `install/`, `cli/src/upgrade/` |
| `server.json` written by new servers; the TCP routes of `cli` and `/server` | `server.rs`, `server/closing.rs`, `web.rs` |

Kept: `rustix`'s `net` feature. `serve.rs:75` shuts down the listener for every inherited descriptor
(a child holds one between `fork` and `exec`). That protection applies to the Unix listener too, so
it is extended, not dropped. Windows stays **explicitly refused** at start (QUA-61) when the
`callers` guard goes.

**What the new binary must answer for a 0.2 upgrader.** (Correction of 2026-11-26: an earlier
revision spoke of a 0.1 upgrader, but release 0.1.0 has no `upgrade` command; it first ships in
0.2.) That upgrader keeps running after it replaced the file. If the unit is active and its `MainPID` runs the replaced file, it runs
`systemctl --user restart quadratique.service`. The unit starts `"<bin>" serve --take-over`, now
the new binary. The upgrader then runs the **new** `server status` until it reports
`running: true`, `sameInterface: true` and `pid` equal to the unit's `MainPID`, within 60 s
(`upgrade/server.rs:194-215`, `:280-300`). So:

| Requirement on the new binary | Why |
| --- | --- |
| `serve --take-over` exists, and works with no server running | the unit runs it |
| the server runs as the unit's main process (no fork into the background) | `pid` must equal `MainPID` |
| `server status` keeps `running`, `sameInterface` and `pid` (the server's own pid), permanently | the old upgrader and the old `install.sh` (`install/install.sh:84-92`) check them |

**Correction (QUA-58, 2026-11-26).** The upgrade code of releases 0.2 and 0.3 does not run the new
`server status`: after it starts the unit again, it waits on its own look at the socket
(`upgrade/server.rs` `ready`, through `background::observe`). That look takes an identity as a
server only when `started` is present and `url` is a loopback address of the command line's routes,
`http://<address>/cli` (`background.rs:660` and `runtime_of`, the same in 0.2.0 and 0.3.0);
anything else is `Malformed`, and the upgrade reports that the new build did not answer. So the new
binary must also keep, on the socket, an identity with `started`, a 32-digit `generation`, `control`
1, the `pid` of the server itself, its `executable`, and a `url` of that form. From QUA-58 on, `url`
is always `http://127.0.0.1:0/cli`: no route answers there, and port 0 is no port, so a command
line of 0.2 or 0.3 that calls it fails at once instead of reaching another program.

That evidence is from the upgrade code on `main`, which first ships in 0.2. Only v0.1.0 is
published, and it has no `upgrade`. The 0.2 path needs evidence from the real 0.2 artifact once it
exists (QUA-55).

| Leftover on an existing machine | What the person reads (README, upgrade notes) |
| --- | --- |
| An enabled unit | Find what it runs and its data home: `systemctl --user cat quadratique.service` (`ExecStart`, `Environment=XDG_DATA_HOME`). Stop and disable it: `systemctl --user disable --now quadratique.service`. Remove the file and reload: `rm ~/.config/systemd/user/quadratique.service`, then `systemctl --user daemon-reload`. The store in `<data home>/quadratique` stays. |
| A copy installed by mise beside the one the unit runs | The same steps. After them, the mise copy is the only one. |
| From the new release on, the unit after a graceful stop | `Restart=on-failure` does not restart after exit 0. The unit stays inactive but enabled, and starts at the next login. That is harmless: it runs the new binary's `serve --take-over`. |
| The reboot recipe (documentation only) | A systemd user unit that runs `quadratique serve` starts at login. With `loginctl enable-linger`, it starts before login. On macOS, a LaunchAgent runs at login. |
| A release that was skipped, or a kernel without pidfds | The manual recovery of §3. |

## 7. Platforms

| | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Commands and MCP | `server.sock` | the same code. Not yet verified: the ACL check (§1, step 3) | named only (QUA-61): a named pipe `\\.\pipe\quadratique-<hash of the data folder>` whose security descriptor admits only the owner's SID, with `PIPE_REJECT_REMOTE_CLIENTS`. tokio's `ServerOptions` sets security attributes only through an `unsafe` pointer, so it needs a safe crate or an owner's decision. Refused at start until then. |
| Web | bearer on `127.0.0.1` | the same | later |
| A Windows browser on a server in WSL | not verified. The bearer no longer depends on the kernel, so the WSL relay is no obstacle in principle. QUA-58 measures it. | — | — |
| Release and install (QUA-36, -37, -39) | binary only | arm64 and x86-64 binaries, now useful | waits for QUA-47 |
| CI exceptions removed | — | the web journeys (QUA-59), the install test (QUA-60 with QUA-36) | all stay until QUA-47 |

No `unsafe` on Linux or macOS: std, tokio and rustix give every call.

## 8. Tests

| What | How |
| --- | --- |
| Another account | As `nobody` (`sudo -n -u nobody`, passwordless on the GitHub Ubuntu and macOS runners), `connect` to `server.sock` gives `EACCES`. The test home lies under traversable ancestors (for example `/tmp/q-<random>` with mode `0755`), so the control case, with the leaf `0755` and the socket `0777`, really connects. That proves the test can fail. |
| Folder policy | A writable non-sticky parent, a link owned by another account (simulated with modes where root is needed), a group-shared leaf, an ACL (Linux), a regular file or a foreign socket at `server.sock`: each is refused before any lock file exists. |
| Path limits | The Rust/tokio API at 107/108 bytes (Linux) and 103/104 (macOS). |
| States | Each row of §2 has a test that reaches it without sleeping: readiness is a `connect` plus the state of the lock, the stop waits on the lock, a stall is a test listener that never answers, the generation decides `409`. |
| Browser | Every row of the route table. A code used twice, a code after 60 s (injected clock), more than 16 pending codes, a cross-origin `POST /session`, `Host: localhost`, a `401` on each data target and on the event stream, a 404 (not the shell) for `/cli/x`. |
| Upgrade | Before and after: E equal, E other, and a `409`. A fake legacy server, with and without the `/proc` evidence. The old upgrader's `server status` contract. |

## 9. Costs

| Part | Change (estimates) |
| --- | --- |
| `quadratique-server` | Removes `peer.rs` (321 lines) and the admission. Adds the home policy, the socket loop, the generation and control protocol, stop, pairing, the route table, the bearer and the bootstrap files. Roughly the same size. |
| `quadratique` (command line) | `background.rs` is rebuilt around §2. `process.rs` keeps `detach`. `upgrade/server.rs` loses `systemctl` and keeps the legacy module (about 150 lines). |
| `install/`, web app | `install.sh` copies a binary. The page gets pairing, `401` handling and the bearer header, and saved views move to the server. |
| Tests and tools to rewrite | The Rust test helpers call over TCP (`server/tests/server/support.rs:117-118`). The browser journeys read `server.json` (`web/e2e/workspace.ts:193`), and their URL helper drops a fragment (`:123-129`). The eval adapter ignores stop errors (`eval/harness/qeval/adapters/quadratique.py:225-226`) and needs short isolated homes. |
| AcidCat | The socket mode, and a narrow release or the move to 0.8. |
| Risk | The pairing and the home policy are new security code. They land as additions while the kernel check still guards TCP, and the cutover is one release. |

| What users lose | |
| --- | --- |
| A tool that called `/cli` on TCP | It uses `curl --unix-socket <home>/server.sock http://quadratique.invalid/cli/...`. |
| Bookmarks | None by default: they keep working across restarts and upgrades. In strict mode they need `ui` after each server start: an old bookmark's port is dead, and the deep link works after `ui`, on the new origin. |
| `ui` over SSH | It prints the remote loopback address. Make a same-port forward before you redeem the code: run `ui` (no `--open`) remotely, then `ssh -L <port>:127.0.0.1:<port> <host>`, then open the link locally within 60 s. A forward of the socket (`ssh -L ./q.sock:<home>/server.sock`) acts as you: README "Who can talk to the server" (`README.md:235-249`) says so. |
| Codex with `network_access = true` | Measured in QUA-53: the socket connects, but the home policy refuses the data folder first, as with the default policy (QUA-84). |
| The installed service | It becomes documentation. A 0.1 unit keeps working. |

Breaking changes, released together as one minor release: QUA-58 (`feat!`) and QUA-60 (`feat!`).

## Order of work

**Additive, before the AcidCat release.** These use the hand-written identity probe
(`server.rs` `ask_identity`), not `HttpClient`.

| Issue | Outcome | Blocked by | Deletes |
| --- | --- | --- | --- |
| QUA-50 | The home policy. The socket beside TCP, with the generation and the control version. `server status` over the socket | — | the `chmod` through a path |
| QUA-54 | Stop and take-over over the socket, with the generation and the `start.lock` handoff. The old SIGTERM path stays for servers without a socket | QUA-50 | nothing |
| QUA-76 | The kept port by default, and the strict setting's fresh origin (carried over from pull request 30) | — | nothing |
| QUA-57 | Pairing and the bearer: one per data folder, its rotation, and strict's bearer per start. Additive (the kernel check still guards TCP) | QUA-50, QUA-76 | nothing |
| QUA-55 | Legacy detection, and the proven legacy stop | QUA-54 | nothing |
| QUA-56 | `upgrade` identifies the server by generation and executable identity. `systemctl` stays until the cutover | QUA-54, QUA-55 | `/proc` matching for new servers |
| QUA-67 | Saved views live in the server | — | the `localStorage` views |

**After AcidCat.**

| Issue | Outcome | Blocked by | Deletes |
| --- | --- | --- | --- |
| QUA-51 | AcidCat: `HttpClient` in Unix-socket mode (AcidCat repository) | — | — |
| QUA-52 | Quadratique moves to that release (narrow 0.6.x if feasible) | QUA-51 | — |
| QUA-53 | Every command, MCP, `init` and `upgrade --skills` use the socket | QUA-50, QUA-52 | the TCP client path |

**The cutover: one release.** QUA-59 is stacked on QUA-58, and QUA-60 on QUA-59. The release
pull request is merged only after all three have landed.

| Issue | Outcome | Blocked by | Deletes |
| --- | --- | --- | --- |
| QUA-58 | TCP serves only the web app, with the bearer required (`feat!`). No `server.json` | QUA-53, QUA-54, QUA-55, QUA-56, QUA-57, QUA-67 | the `cli` target and `/server` on TCP, `localhost`, `server.json` |
| QUA-59 | The kernel check is deleted. macOS serves. Windows is refused explicitly | stacked on QUA-58 | `peer.rs`, the admission, the pid paths outside the legacy module, a CI exception |
| QUA-60 | The installer is the binary only. `upgrade` loses `systemctl`. The README covers the unit and the reboot recipe (`feat!`) | stacked on QUA-59 | the unit, `uninstall.sh`, the fake `systemctl`, `systemctl` in `upgrade` |

**Afterwards.** QUA-62: the PRD (§6, §15, M7, M8), blocked by QUA-59 and QUA-60. QUA-61: Windows,
blocked by QUA-47.

**Pull request 30.** It makes the port of today's listener persistent, and that listener still
carries the `cli` routes. GitHub reported it `MERGEABLE` at head `3a4634d` when this revision was
written; the review's statement that it conflicts with `main` no longer holds. Pull request 30 was
closed unmerged. After the owner's answers, QUA-76 carries its kept port onto `main` as the default
(2026-11-26). Its `bind → publish → accept` ordering fix exists for `server.json`, which the cutover
removes, and was not carried over.

| Existing issue | Becomes (state left to the lead) |
| --- | --- |
| QUA-33, QUA-34 | replaced by QUA-50 … QUA-59, and by QUA-61 |
| QUA-46 | ended by QUA-59 |
| QUA-63 (W2: the command line trusts the port and its pid) | ended by QUA-53 (commands over the verified socket), QUA-54 and QUA-55 (no unproven signal), and the cutover |
| QUA-43 | cancelled, and replaced by QUA-76. Pull request 30 is closed |
| QUA-6, QUA-36, QUA-37, QUA-38, QUA-39 | stay, simpler (binary only). Windows waits for QUA-47 |

## Open questions for the owner

All three are answered (2026-11-26); decisions 7 to 9 above record the answers.

1. **Bearer lifetime.** Asked: one bearer per server start? Answer: no. One bearer per data folder,
   valid across restarts until rotated; per server start only in strict mode.
2. **Kept port.** Asked: opt-in, with a fresh origin by default? Answer: no. The port is kept by
   default; the fresh origin is strict mode's.
3. **AcidCat.** Asked: a native Unix-socket mode in AcidCat's Rust `HttpClient`? Answer: yes,
   acidcat#235.
