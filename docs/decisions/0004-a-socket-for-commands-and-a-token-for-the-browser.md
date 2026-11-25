# 0004. A socket for commands, a token for the browser, and no service

Date: 2026-11-25

Status: proposed. Supersedes [0003](0003-the-caller-check-on-macos-and-windows.md), and replaces
the Linux kernel check that 0003 kept.

## Context

The server has one TCP listener on `127.0.0.1`. The command line, `quadratique mcp` and the
browser all use it. On Linux the server asks the kernel which account owns each connection
(`server/src/peer.rs`, `NETLINK_SOCK_DIAG`). Elsewhere it refuses to start. `install.sh` writes a
systemd user unit. `quadratique upgrade` restarts that unit, or stops the server by sending a
signal to its process number.

Reviews found these weaknesses in that design:

| # | Weakness |
| --- | --- |
| W1 | While the server is down, another account can bind its port. A bookmark then sends its requests to that account, and that account's page can read the web app's `localStorage` for the origin. |
| W2 | The command line trusts whoever answers on the port, then acts on the process number that the answer gives. |
| W3 | The kernel lookup identifies a connection by its addresses and ports, which can be reused (QUA-46). |
| W4 | `quadratique upgrade` must find and signal the right process, and must know about systemd. |

The owner decided on 2026-11-25 (QUA-48). These points are fixed:

1. The command line and the MCP server reach the server through a Unix socket in the private
   data folder. File permissions decide who can connect, in both directions. They use no TCP
   port and no kernel lookup.
2. The Linux kernel check goes too. Every system uses the same mechanism.
3. Only the web app uses TCP. It requires a token that `quadratique ui` gives to the page. The
   page saves the token in the browser.
4. The systemd service is no longer part of the product. Installing means putting one binary on
   the PATH. The documentation explains how to keep the server running after a reboot.
5. Stop and take-over are requests on the socket. They are not signals to a process number.
6. Windows waits for QUA-47. This record names the Windows equivalent and builds nothing for it.

Facts measured for this record, on Linux 7.2 with a throwaway probe that was not committed:

| Probe | Result |
| --- | --- |
| `connect` with no socket file | `ENOENT` |
| `connect` to a socket file whose listener closed | `ECONNREFUSED`, and the file stays |
| A second `bind` on an existing socket file | `EADDRINUSE` |
| Mode of a new socket under umask 022 | `0755` |
| Path of 107 bytes, then of 108 bytes | bound, then "AF_UNIX path too long" |
| The account `nobody` connects. Folder `0755`, socket `0600` | `EACCES` |
| The same. Folder `0700`, socket `0777` | `EACCES` |
| The same. Folder `0755`, socket `0777` | connected |

Facts measured earlier: a plain Claude Code shell can connect to a `0600` socket in the data
folder. Inside `codex sandbox`, both a Unix socket and loopback TCP fail. Any account on the
machine can reach loopback TCP.

## What AcidCat gives

| Side | Finding (acid-cat-core 0.6.0, `~/.cargo/registry/src/index.crates.io-*/acid-cat-core-0.6.0/src/`) | Consequence |
| --- | --- | --- |
| Server | `remote::serve` binds `TcpListener` (`remote.rs:323-324`). But `Handler` (`remote.rs:33`) and `Routes` (`remote.rs:320`) work on `http::Request`, with no transport. Quadratique already has its own accept loop (`server/src/serve.rs`), because `remote::serve` hides the peer. | The server needs no change in AcidCat. `serve.rs` gets a `UnixListener` loop, and hyper serves a `UnixStream` through `TokioIo` in the same way. |
| Client | `HttpClient` always dials TCP: `client::connect` calls `TcpStream::connect` (`client.rs:36`), and that function is `pub(crate)`. `CliOptions::remote` accepts only an `HttpClient` (`cli.rs:295`, `cli.rs:336`). AcidCat `main` (0.8.0, `packages/core-rust/src/client.rs:36`) is the same. | The command line cannot reach a socket without a change. |
| Web client | `@acid-cat/core-effect` 0.13.1 sends calls and subscriptions with `fetch` and takes `headers` (`dist/internal/httpClient.d.ts`, `ClientOptions.headers`). It does not use `EventSource`. | The live channel can carry an `Authorization` header. 0003's concern about `EventSource` does not apply. |

The two ways to give the client a socket:

| Option | Size | Cost |
| --- | --- | --- |
| **A. AcidCat: `HttpClient` gets a Unix-socket setting** (QUA-51) | About 60 lines in `client.rs` and `http_client.rs`, plus a test, under `cfg(unix)`. | A release of AcidCat. Quadratique moves from `=0.6.0` to that release, and that range has at least one breaking change (acidcat #215). The size of that upgrade was not measured (QUA-52). |
| B. Quadratique: its own client below the generated command line | `CliOptions::remote` accepts only `HttpClient`. So Quadratique would have to run the command line against a local application whose every capability forwards over its own client. That means writing again `invoke`, `capabilities` and the SSE `subscribe` parser (`http_client.rs` has 497 lines), plus the MCP upstream. | A second copy of AcidCat's client, which then drifts from AcidCat. |
| (C. A loopback relay to the socket) | — | Rejected: it brings TCP back for commands. |

**Recommendation: A.** If the upgrade to 0.8 is large, ask for a 0.6.x release that has only
this setting.

## Decision

### 1. The socket

| Question | Decision | Rejected, and why |
| --- | --- | --- |
| Path | `<data folder>/server.sock`. The data folder is `QUADRATIQUE_HOME` or the platform's default, as today. Its name matches `server.lock` and `server.log`. | A socket in `$XDG_RUNTIME_DIR` or `$TMPDIR`: decision 1 puts it in the data folder. |
| Modes | The folder is `0700` and is owned by the server's account. The server already sets `0700` (`server.rs` `private`). It now also checks the owner and the mode, and refuses to start if they are wrong. After `bind`, the server sets the socket to `0600`. The probe shows that each layer alone keeps out another account on Linux. The folder is the guarantee on every system. | Relying on the socket's mode alone: the umask makes it `0755`, and macOS's rule for socket modes was not checked. |
| Client side | Before it connects, the command line checks that the folder is owned by its own account and has no group or other bits. If the check fails, it refuses with "the data folder is not private". So only this account (or root) can have bound the socket that answers. This closes **W2**. | Peer credentials (`UnixStream::peer_cred`): decision 1 makes file permissions the rule. |
| Stale socket file | Only the holder of `server.lock` removes `server.sock`, and it then binds right away. A client never removes it. | Removing the file on `ECONNREFUSED` from a client: it races with a server that is starting. |
| Path too long | `sun_path` has 108 bytes on Linux (107 usable, probed) and 104 on macOS (103 usable, from `<sys/un.h>`, not probed here). If the path is longer, the server and the client refuse. The message gives the path, its length, the limit, and "set `QUADRATIQUE_HOME` to a shorter folder". The default macOS path is `/Users/<name>/Library/Application Support/quadratique/server.sock`, 59 bytes plus the name. On macOS, tests must keep temporary folder names short. | `chdir` and a relative path: the working directory belongs to the whole process, and the server has many threads. `/proc/self/fd/N/server.sock`: Linux only. The Linux abstract namespace: Linux only, and it has **no file permissions**, so any account could connect. |
| File system without sockets | The server refuses to start. The message gives the folder and the error. | Falling back to TCP: that would open W1 and W2 again. |
| Two servers | No change: `server.lock` (`flock`) still lets only one server own a home, and the server binds only while it holds that lock. `start.lock` still lets only one command start a server at a time. | — |
| Home on a network file system | Not verified. Expected behaviour: the socket file exists, but `connect` reaches only a listener on the same machine. Linux NFSv4 maps `flock` to a lock that every machine sees. A second machine then sees the home held and the socket refusing, so it refuses, and the message names the other machine as possible. Today's design is worse: `server.json` points the second machine at its own loopback. | — |

**How the command line reads what it finds.** `server.json` no longer helps find the server: the
socket path is fixed.

| `connect` gives | `server.lock` | Meaning | The command line |
| --- | --- | --- | --- |
| `ENOENT` or `ECONNREFUSED` | free | no server | starts one, under `start.lock`, as today |
| `ENOENT` or `ECONNREFUSED` | held, no `server.json` | a server is starting or stopping | waits on the lock and the socket, up to `START_LIMIT`, then reports `NoAnswer` |
| `ENOENT` or `ECONNREFUSED` | held, and a `server.json` exists | a server of release 0.1 or 0.2, on TCP | uses the path in §5 |
| connected, `GET /server` answers in time | — | the server of this home (interface and schema as today) | uses it |
| connected, no answer within `PROBE_LIMIT` | — | a server that does not answer | reports it, and acts on no process |
| `EACCES` or `EPERM` | — | a sandbox, or another account | `Unreachable`, with the MCP advice as today |

`server.json`: new servers do not write it. A new server deletes a stale one while it holds the
lock. Only the path for earlier releases reads it (§5). `GET /server` on the socket returns
`pid` (for diagnosis only), `database`, `interface`, `schema`, the web address or `null`, and
`started` (`on-demand` or `serve`).

### 2. The lifecycle with no service

| Question | Decision | Rejected, and why |
| --- | --- | --- |
| Start | Any command that needs the server starts it, as today. `quadratique serve` stays as the way to run the server in the foreground, for a terminal or for a service manager that the person sets up. | — |
| Stop when idle | **No.** The server runs until `server stop`, a signal, logout or reboot. | An idle timeout: the web app would lose its live updates and its token (§3) after each idle period. Merges would be seen only when the next command starts the server. One idle process costs little. |
| Git watcher | No change: it looks every 2 s while the server runs, and a read asks for a look. After a reboot, merges are seen when the next command starts the server, as today on a machine without the service. | — |
| Stop | `POST /server/stop`, on the socket only. The server answers, then closes its listeners and its store and frees `server.lock`. `server stop` waits on that lock. A file lock is a state, not a timer. The limit only stops a hang. This closes **W4** for stopping. | A signal to the process number that `server.json` gives: that was W2 and W4. |
| Take-over | `serve --take-over` sends the stop request, waits for the lock, then serves. | — |
| Signals | The server still stops on SIGTERM and SIGINT (Ctrl-C, service managers). Only the command line stops sending signals. | — |
| `quadratique upgrade` | It replaces the binary as today, then sends the stop request. The next command starts the new build. If `started` is `serve`, it says: "this server was started with `quadratique serve`; restart it the way you started it." It no longer runs `systemctl`, no longer reads `/proc/<pid>/exe`, and no longer uses a pidfd. | Restarting a service: decision 4. |

### 3. The web app on TCP

| Question | Decision | Rejected, and why |
| --- | --- | --- |
| Routes | TCP serves `/`, the static files, `/session`, `/web` and `/webmcp`. The `cli` target and `/server` are served only on the socket. | — |
| When the listener exists | From the first pairing request (`quadratique ui`) until the server stops. A machine where only agents use Quadratique has no TCP port. | Always on: it opens a port that nobody uses. Under a per-start token, a bookmark needs `ui` after each start anyway. |
| Port | Any free port, unless the person turns on the kept-port setting of pull request 30 (QUA-43). With it, the server asks for the last port first and falls back to any free port. The setting applies to this listener only. | — |
| Host | Only `127.0.0.1:<port>`. A `Host: localhost:<port>` request is refused, and the server gives out only `127.0.0.1` URLs. Reason: a browser can resolve `localhost` to `::1`, and another account can bind `[::1]:<port>` **while the real server runs**. That account would then get a page at an origin that the person thinks is theirs. Today `web.rs` (`Origin::hosts`) accepts `localhost`. | Binding `::1` as well: it adds a second listener only to support a name that nobody needs. |
| Token | 32 random bytes in base64url. The server makes it at start, keeps it in memory, and never writes it to disk. It ends when the server stops. See the threat analysis below. | A token stored in the data folder: see below. |
| Handover | `quadratique ui` asks the server, over the socket, for a **one-time code** (single use, valid 60 s, in memory). With `--open`, the server writes `<data folder>/open-<random>.html` (mode `0600`) with `<meta http-equiv="refresh" content="0;url=http://127.0.0.1:<port>/#pair=<code>">`, and opens that file. Only the file path appears in process arguments, which `ps` shows to every account. The server deletes the file when the code is used or expires. Without `--open`, `ui` prints the link with the code, for browsers that cannot read the data folder (snap or flatpak confinement) and for remote terminals. | A token or a code in the URL given to `xdg-open` or `open`: another account can read it with `ps` and use it first. A `file://` page that posts the code itself: its origin is `null`, and it cannot write the storage of `http://127.0.0.1:<port>`. |
| History and `Referer` | The code is in the fragment, which the browser sends neither to the server nor in `Referer`. The page posts the code to `/session`, receives the token, and removes the fragment with `history.replaceState`. If a browser's global history still keeps the first URL, the code in it is already used. | — |
| Storage in the browser | `localStorage` at the origin, as the owner asked. The origin includes the port, so a new port means a new pairing. | A cookie: browsers send it to every port of `127.0.0.1` (0003). `sessionStorage`: it is lost when the tab closes, against the owner's "save the token". |
| Every request | `Authorization: Bearer <token>` on `/web`, `/webmcp` and the live channel, through the core-effect client's `headers` option (`web/src/core/server.ts`). | — |
| Without the token | The static files are served (they hold no data). `/web` and `/webmcp` answer `401` with "Run `quadratique ui --open`", and the page shows that message. | — |
| WebMCP and `web` | No change in who the caller is: `web` is the human, and `/webmcp` is the agent `browser`. Both need the token. | — |

**Threat analysis.** Here another account B on the same machine wants the person's issues. Every
attack below needs B. A machine with a single account has no B.

| While the person's server is down, B binds the web port and serves a page. The person opens a bookmark, and B's page reads that origin's `localStorage`. | What B gets | What B can then do |
| --- | --- | --- |
| Token stored in the data folder, so it survives restarts | a valid token | Use it with `curl` against the real server, on whatever port that server has later, as the human, until the token is revoked. **Rejected.** Without a kept port this still happens: B sees with `ss` which ports the server listened on, and binds an old one that a bookmark still names. |
| **Token per server start (recommended)** | a token that no longer works | Nothing with the token. B can still show a fake board, read what the person types into it, and read the saved views (`web/src/main.ts`). |
| Token bound to the browser: a non-extractable WebCrypto key in IndexedDB, and a challenge from the server bound to the server's current origin | a key that B can use only inside the person's browser, at an origin where the real server is not | Nothing with the key. The fake board stays possible. Cost: a key registry in the data folder, a challenge route, IndexedDB code, and revocation, about 300 to 500 lines. This is the only option whose pairing survives server restarts safely. Do it later if pairing again after a restart annoys the owner. |

| Port choice | Effect on the fake-board attack | Cost |
| --- | --- | --- |
| New port at each start (default) | B must bind the specific old port that a bookmark names. | Bookmarks and open tabs stop working at each restart. |
| Kept port (opt-in) | Every bookmark names the one port that B can take while the server is down. | None for the person while no other account exists. |

**Recommendation.** On a machine with a single account, use a per-start token with the kept port
turned on. On a shared machine, use a per-start token and leave the kept port off, which is the
default. With either choice, the person runs `quadratique ui` once after each server start.
Because the server no longer stops when idle (§2), starts happen at reboot, upgrade, or
`server stop`.

What this closes: **W1** for the token, but not the fake board (that stays open and is
documented). **W2** and **W3**: TCP no longer carries the `cli` target, and no lookup exists.

### 4. Remote access (PRD §6 and M7)

The plan does not change, and one rule becomes clearer. "On the machine that runs the server"
now means "over the socket". `token.*`, `session.approve`, `session.list`, `session.revoke` and
`issue.advance` stay on the `cli` target, which only the socket serves. The tailnet board and
MCP on the internet each keep their own backend and their own credential (a tailnet session, an
agent token). The local web token is not valid there. The tailnet pairing can use the same
one-time code, approved over the socket.

### 5. What is deleted, and what stays for an upgrade from 0.1 or 0.2

| Deleted | Where |
| --- | --- |
| The kernel lookup, the admission, the refusal drain, `CallersUnknown`, the refusal to start on macOS, and the `rustix` `net` dependency | `server/src/peer.rs`, `server.rs` (`callers`), `serve.rs`, `server/Cargo.toml` |
| Signals and process numbers in the command line (`ask_to_stop`, `alive`, pidfd) | `cli/src/process.rs`, `cli/src/background.rs`, `cli/src/upgrade/server.rs` |
| The service: the unit that `install.sh` writes, `uninstall.sh`, the fake `systemctl` in `install/test.sh`, the `systemctl` code in `upgrade`, and the service hint for copies installed by mise | `install/`, `cli/src/upgrade/{server.rs,mod.rs}` |
| `server.json` for new servers | `server.rs`, `server/closing.rs` |

| Kept for an existing machine | Why, and until when |
| --- | --- |
| The `server status` fields `running`, `sameInterface` and `pid` | The `install.sh` and `upgrade` of 0.1 and 0.2 run the **new** binary's `server status` and check these fields after they replace the binary. Keep them permanently: they are cheap. |
| `quadratique serve --take-over` | The installed 0.1 or 0.2 unit runs `"<bin>" serve --take-over`. It keeps working with the new binary and becomes the documented recipe. |
| One module that finds a 0.1 or 0.2 server (`server.lock` held, no socket, a `server.json` with an address and a pid), asks for its identity over TCP, and stops it with SIGTERM through a pidfd. This is today's code. Linux only, because those releases served only there. | Without it, a server of an earlier release would hold the home with nothing that can stop it. It still trusts the port, as W2 describes. Delete it two minor releases later (QUA-55). |

**The installed unit.** The product no longer removes it, because that would mean knowing about
systemd again. The upgrade notes and the README say: the unit still works and now runs the new
binary, and `systemctl --user disable --now quadratique.service` followed by deleting the file
removes it. Under `Restart=on-failure`, a stop request gives exit status 0, so systemd does not
start the server again. The next command starts it on demand.

### 6. Platforms

| | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Commands and MCP | `server.sock` | `server.sock`, same code (std and tokio `UnixListener` and `UnixStream`, `PermissionsExt`) | Named only, waits for QUA-47 (QUA-61): a named pipe `\\.\pipe\quadratique-<hash of the data folder>` with a security descriptor that admits only the owner's SID, and `PIPE_REJECT_REMOTE_CLIENTS`. tokio's `ServerOptions` accepts security attributes only through an `unsafe` raw pointer, so QUA-61 needs a safe crate or an owner's decision about `unsafe`. AF_UNIX on Windows stays rejected until its permission check on `connect` is verified (0003). |
| Web | token on `127.0.0.1` | same | same, later |
| Release binaries (QUA-36) | as today | arm64 and x86-64, now useful because the server serves on macOS | waits for QUA-47 |
| Install (QUA-37, QUA-39) | download, check, put the binary on the PATH; no unit | same | QUA-38, waits for QUA-47 |
| CI exceptions removed | none | "Web journeys (expected to fail until QUA-33, QUA-34)" when QUA-59 lands; the macOS part of "Install test (expected to fail …)" when QUA-60 and QUA-36 land | all stay until QUA-47 |

`unsafe`: Linux and macOS need none. Every call goes through std or tokio.

### 7. Tests

| What | How |
| --- | --- |
| Another account cannot connect | A test runs `connect` to `server.sock` as `nobody` (`sudo -n -u nobody`, which is passwordless on the GitHub Ubuntu and macOS runners) and expects `EACCES`. It runs in CI on both systems and is not an ignored test (QUA-50, QUA-59). A second case loosens the folder and the socket to `0755` and `0777` and expects a connection, which shows that the test can fail. |
| The folder must be private | Unit tests: the server refuses to start, and the client refuses to connect, when the folder has group or other bits. |
| The token is required | Integration tests: `/web` and `/webmcp` and the live channel without a token, with a wrong token, and with the token give 401, 401 and 200. `/cli/*` and `/server` on TCP give 404. `Host: localhost` is refused (QUA-58). |
| The code works only once | Tests: a second use of the code, and a use after 60 s, are refused. The 60 s run on an injected clock (QUA-57). |
| Nothing depends on timing | Readiness is the result of `connect` together with the state of `server.lock`. The stop waits on the lock. Code expiry uses an injected clock. No test sleeps to wait for a state. Limits only stop hangs. |
| Upgrade from an earlier release | A fake 0.2 server (a TCP listener that answers `GET /server`, holds `server.lock` and has a `server.json`): a command reports it, and `server stop` ends it (QUA-55). |

### 8. Costs

| Part | Change (estimates, not measured) |
| --- | --- |
| `quadratique-server` | It loses `peer.rs` (321 lines), the `callers` modules and the refusal drain. It gains the socket loop, the folder check, the routes split between socket and TCP, the stop route, pairing, the token check and the bootstrap file. The net size is about the same. |
| `quadratique` (command line) | `background.rs` gets simpler (the `connect` result replaces the TCP probes and the pid handling). `process.rs` keeps only `detach`. `upgrade/server.rs` loses most of its 347 lines. The legacy module is about 100 lines. The net size is clearly smaller. |
| `install/` | `install.sh` copies a binary. `uninstall.sh` goes away. `test.sh` shrinks. |
| web app | Pairing, storage, the header and the 401 screen: about 150 lines plus tests. |
| AcidCat | About 60 lines and a release. Quadratique moves from 0.6 to 0.8 (size unknown, QUA-52). |
| Risk | The highest risk is the web pairing (new security code: one-time code, bootstrap file, token check). It lands as an addition while the kernel check still guards TCP (QUA-57), and the token becomes required only in QUA-58. The kernel check is deleted last (QUA-59). |

| What users lose | |
| --- | --- |
| A tool that called `/cli` on the TCP port | It must use `curl --unix-socket <data folder>/server.sock http://localhost/cli/...`. |
| Bookmarks and open tabs | They need `quadratique ui` once after each server start, and after each port change when the kept port is off. |
| `codex` with `network_access = true` | Not measured for a Unix socket. QUA-53 measures it and updates "Agents in a sandbox". |
| The service that kept the server running after a reboot | It becomes a recipe in the README (QUA-60). A 0.1 or 0.2 unit keeps working. |

**Compatibility.** QUA-58 (TCP stops carrying `/cli` and requires a token) and QUA-60 (the
installer writes no unit) are breaking changes, so their pull request titles take `!`
(`feat!:`). Before 1.0.0, a breaking change makes a minor release.

## Order of work

QUA-50 comes first. It is the end-to-end proof, needs no AcidCat change, and deletes nothing.

| Issue | Outcome | Blocked by | Deletes |
| --- | --- | --- | --- |
| QUA-50 | The server also listens on `server.sock`, and `server status` uses it | — | nothing |
| QUA-51 | AcidCat's `HttpClient` can reach a Unix socket (in the AcidCat repository) | — | nothing |
| QUA-52 | Quadratique depends on that AcidCat release | QUA-51 | nothing |
| QUA-53 | Every command and `quadratique mcp` use the socket | QUA-50, QUA-52 | the TCP client path of commands |
| QUA-54 | Stop and take-over are requests on the socket | QUA-50 | signals and pids in the command line |
| QUA-55 | The command line finds and stops a 0.1 or 0.2 server | QUA-54 | nothing (a module to delete later) |
| QUA-56 | `upgrade` asks the server to stop and knows no service | QUA-54, QUA-55 | `systemctl`, `/proc/<pid>/exe` and pidfd in `upgrade` |
| QUA-57 | `quadratique ui` pairs the page, and the page sends the token (not yet required) | QUA-50 | nothing |
| QUA-58 | TCP serves only the web app, with the token (`feat!`) | QUA-53, QUA-54, QUA-57 | the `cli` target and `/server` on TCP, `localhost` |
| QUA-59 | The kernel check and the macOS refusal are deleted | QUA-58 | `peer.rs`, admission, drain, `CallersUnknown`, a CI exception |
| QUA-60 | The installer puts only the binary on the PATH, and the README has the reboot recipe (`feat!`) | QUA-56 | the unit, `uninstall.sh`, the fake `systemctl` |
| QUA-61 | Windows: a named pipe for the owner only (named, not built) | QUA-47 | — |
| QUA-62 | The PRD describes this design | QUA-59, QUA-60 | the old text in §6 and §15 |

Existing issues that this plan affects. Their state is left for the lead to change:

| Issue | Becomes |
| --- | --- |
| QUA-33 macOS caller check | replaced by QUA-50 to QUA-59 (pull request 28 is already closed) |
| QUA-34 Windows caller check | replaced by QUA-61 |
| QUA-46 the decision outlives the connection | closed by QUA-59 (no lookup remains) |
| QUA-6 install on every platform | stays as the umbrella. Its service part is replaced by QUA-60 |
| QUA-36 release binaries | stays. It is useful for macOS after QUA-59. Windows waits for QUA-47 |
| QUA-37 one-command install | stays, simpler: binary only, after QUA-60 |
| QUA-38 Windows install | stays, waits for QUA-47 |
| QUA-39 README per platform | stays. The reboot recipe is in QUA-60 |
| QUA-43 kept port (pull request 30) | stays as an opt-in setting that applies to the web listener only. It can land before or after QUA-58. If it lands before, QUA-58 moves its listener |

## Open questions for the owner

Each question has a safe default, which applies until the owner decides.

1. **How long the token lasts.** Recommendation: per server start, so a stolen token stops
   working. The page pairs again after a restart. Confirm, or ask for the browser-bound key
   (about 300 to 500 lines, and the pairing survives restarts) as a later issue. A token stored
   in the data folder is not offered.
2. **The kept port.** Recommendation: keep it opt-in and off by default, and turn it on for a
   machine with a single account. Confirm the default.
3. **The change in AcidCat.** Recommendation: option A (QUA-51), about 60 lines in your AcidCat
   repository and a release, rather than a second client in Quadratique. Confirm, and say
   whether a 0.6.x release is acceptable if moving Quadratique to 0.8 turns out large.
