# 0003. The caller check on macOS and Windows

Date: 2026-11-25

## Context

The server listens on one TCP port of `127.0.0.1`. The command line, the MCP stdio server and
the owner's browser all reach it there. The server admits a connection only when it comes from a
process of the account that runs the server. A process of another account on the same machine
gets `403` (the owner-only answer, `serve::OWNER_ONLY`), on every route. This is a security
property: the store holds every issue of every repository of the owner.

On Linux the check is `server/src/peer.rs`. For each accepted connection the server asks the
kernel, through a `NETLINK_SOCK_DIAG` request, for the uid of the socket at the other end of the
connection. The kernel records that uid when the socket is created, so the answer does not depend
on a process id that can be reused. Before it serves, the server looks up one connection of its
own (`check_own_connection`) and refuses to start if the kernel cannot answer or names another
uid. The check fails closed: a lookup that fails refuses the connection.

Until this record, a build for another platform compiled the check out and admitted every
connection (`server/src/server.rs`, the non-Linux `callers`). That build was never released, but
it shows the risk: "it compiles" is not "it is safe". QUA-32 replaced it with a refusal to start.
The message names this record's issues.

The owner wants Quadratique to install on macOS and Windows, and WSL to behave as Linux. This
record chooses how the server identifies its callers on macOS and on Windows. It also chooses
what replaces the systemd user service there. QUA-32 does not implement either. QUA-33 (macOS)
and QUA-34 (Windows) implement them.

Constraints that every candidate must meet:

1. A process of another account is refused, as strictly as on Linux. A failed lookup refuses.
2. The owner's browser reaches the web app on the same machine. A browser speaks HTTP over TCP
   only: it cannot open a Unix domain socket or a named pipe.
3. The workspace forbids `unsafe` code (`unsafe_code = "forbid"` in `apps/quadratique/Cargo.toml`).
   Linux needs none, because `rustix` wraps the system calls. On macOS and Windows the system
   interfaces are C functions. Each candidate either finds a maintained crate with a safe
   interface, or the workspace lowers `forbid` to `deny` and allows `unsafe` in one reviewed
   module.

## Candidates

### A. Keep TCP, and ask the system who owns the other socket

The design stays as on Linux. Only the lookup changes per platform.

- **macOS.** No system call maps a TCP connection to a uid. `libproc` lists the sockets of a
  process: `proc_listpids` with `PROC_UID_ONLY` and the owner's uid lists the owner's processes;
  `proc_pidinfo(PROC_PIDLISTFDS)` and `proc_pidfdinfo(PROC_PIDFDSOCKETINFO)` give each socket's
  local and remote address. This is what `lsof` does. The server admits the connection when one
  of the owner's processes holds the socket whose local address is the connection's peer
  address. macOS answers `proc_pidfdinfo` only for processes of the same uid, unless the caller
  is root. So a socket of another account is never found, and the connection is refused. That
  is the fail-closed property, but it rests on the kernel's access policy. QUA-33 must prove it
  with a test in which a second account connects.
  - Cost: one scan of the owner's processes and their descriptors for each new connection. A
    few hundred processes take some milliseconds. HTTP keep-alive spreads the cost, and the
    command line opens one connection per command. QUA-33 measures it.
  - Safe interface: the `libproc` crate wraps these calls.
  - Gap: a process of the owner may hand its socket to a process of another account (through
    `SCM_RIGHTS`). The owner chose to do that. Linux has the same gap, because the socket's uid
    stays the creator's.
- **Windows.** `GetExtendedTcpTable` with `TCP_TABLE_OWNER_PID_ALL` gives the process id that
  owns each TCP connection, for every account, with no privilege. The server looks up the
  connection whose local address is the peer's address and reads its pid. It then opens that
  process (`OpenProcess` with `PROCESS_QUERY_LIMITED_INFORMATION`, then `OpenProcessToken` and
  `GetTokenInformation(TokenUser)`) and compares the token's user SID with its own. Windows
  refuses `OpenProcessToken` on another account's process to a non-elevated caller. That
  failure refuses the connection.
  - Gap: the table gives a process id, not an owner that the kernel records on the socket as on
    Linux. If the connecting process exits and the system gives its pid to a new process of the
    owner before the lookup, the owner's token would be read. The socket of the process that
    exited is then closed, unless that process gave a copy of it to a child. A copy belongs to
    the same account that made the connection. QUA-34 must write this down and test the
    common case: another account is refused.
  - Safe interface: no maintained crate gives these four calls safely. The `windows` crate
    declares them `unsafe`. So QUA-34 needs one module that may use `unsafe` (the workspace's
    `forbid` becomes `deny`, and the module says `allow` with a comment for each call). That is
    an owner's decision. The open questions below list it.
- **Browser.** No change. The browser connects over TCP like any other process of the owner,
  and the lookup finds it. `quadratique ui` opens the URL as it does now.

### B. A Unix domain socket or a named pipe for commands, and TCP for the browser

- **macOS.** A Unix domain socket in the owner's 0700 data folder. The kernel gives the peer's
  uid with `getpeereid` (or `LOCAL_PEERCRED`). The folder's permissions already stop other
  accounts from connecting. This is the strongest check for the command line.
- **Windows.** A named pipe whose security descriptor admits only the owner's SID. The kernel
  refuses to open the pipe for another account. The server can also read the client's pid with
  `GetNamedPipeClientProcessId`. Windows has `AF_UNIX` sockets too, but they give no peer
  credentials (`SO_PEERCRED` is not supported). Only the socket file's access rights would
  protect them, and this record has not verified how Windows checks those rights on `connect`.
- **Browser.** It cannot use either. The server keeps a TCP listener for the web app, and that
  listener needs its own check. Without candidate A, the only check left is a secret
  (candidate C). So B is A or C for the browser, plus a second transport for the command line.
- Cost: two transports and two code paths in the server and in the command line (the client in
  `cli/src/route.rs`, the discovery through `server.json`, the probes in `cli/src/background.rs`).
  Every test that starts a server would run both paths.

### C. A secret that only the owner can read

The server writes a random secret to a file in its data folder, readable only by the owner. The
command line reads the file and sends the secret with each request. The server refuses a
request without it. The same scheme could run on Linux, so all platforms would share one check.

- Guarantee: as strong as the permissions of the file. On macOS and Linux that is mode 0600 in a
  0700 folder (the server already sets both: `server.rs` `private`, `credentials.rs`
  `new_private_file`). On Windows the server would have to set an owner-only ACL itself.
  `private` does nothing on Windows today. The default folder, `%LOCALAPPDATA%`, inherits the
  profile's ACL (the owner, SYSTEM and Administrators), but a `QUADRATIQUE_HOME` elsewhere may
  not.
- **Browser.** The browser must get the secret, and it must not leak to other accounts:
  - In the URL that `quadratique ui` opens: the URL is an argument of `open`, `xdg-open` or the
    Windows opener. Other accounts can read process arguments (`ps` on macOS and Linux). A
    single-use code that the server exchanges for a session at once narrows the window, but
    does not close it: another account can watch for the code and use it first.
  - In a file: `quadratique ui` writes a page with the code into the 0700 folder and opens the
    page's `file://` path. The page posts the code to the server. Only the path is visible. The
    request then comes from the origin `null`, which the server's origin guard (`web.rs`
    `SameOrigin`) refuses today.
  - Keeping the session: a cookie for `127.0.0.1` is sent to every port of `127.0.0.1`. Cookies
    are not separated by port. A server of another account on another port would receive it if
    the owner's browser visited that port. So the web app would have to keep the session in
    `sessionStorage`, which is separated by port, and send it as a header. `EventSource`, which
    the live board uses, cannot send headers. It would have to become a streamed `fetch`.
- Cost: the bootstrap page, the change of the origin guard, a new session in the web app, and
  the Windows ACL. Each is security code that the other candidates do not need.
- Benefit: it also serves an agent on another machine, which the agent tokens of
  `credentials.rs` prepare for. That is a different problem: there the caller is not on this
  machine, and no kernel can vouch for it.

### What each candidate gives

| | Another account refused by | Browser | New code | `unsafe` |
| --- | --- | --- | --- | --- |
| A, macOS | the kernel's rule that only the same uid may read a process's sockets | unchanged | a lookup through `libproc` | no (`libproc` crate) |
| A, Windows | the kernel's rule on another account's process token | unchanged | a lookup through four Win32 calls | one module |
| B | the socket's or the pipe's permissions, checked by the kernel | needs A or C as well | a second transport, everywhere | Windows pipe security: one module |
| C | the file's permissions | a bootstrap page, a new session, an origin-guard change | the secret, the session, the Windows ACL | Windows ACL: one module |

## Decision

1. **Candidate A on both platforms.** The server keeps one TCP listener and asks the system who
   owns the other end of each connection: `libproc` on macOS (QUA-33),
   `GetExtendedTcpTable` and the process token on Windows (QUA-34). Each lookup fails closed. As
   on Linux, the server looks up a connection of its own before it serves, and refuses to start
   if the lookup does not name its own account.
2. **Until a platform has its lookup, the server refuses to start there** with a message that
   names the issue. QUA-32 did this. No build serves without a caller check.
3. **Each lookup is proved by a test in which another account connects and gets `403`**, in CI
   where the runner allows a second account (the macOS and Windows runners do: `sysadminctl` and
   `net user` with the runner's administrator rights), as `test-as-nobody` does by hand on Linux.
4. **No service manager on macOS and Windows for now.** The server already starts on demand:
   the first command that needs it starts it in the background (`cli/src/background.rs`). On a
   machine without a systemd user manager (macOS, Windows, WSL without systemd) the installer
   installs the binary and says that the server starts when a command needs it. A launchd
   agent on macOS or a task at logon on Windows (a Windows service runs under another account,
   so it is the wrong tool for a per-user server) is worth adding only when something needs
   the server before any command runs. Nothing does today: `quadratique ui` starts it, and
   the port changes at each start anyway, so a bookmark could not rely on a running service.
   WSL with systemd uses the Linux installer and its user service.

Why A: it is the design that Linux already proves, so the server, the command line, the web app
and their tests keep one transport and one model of the caller. The browser needs no secret,
and no secret can leak. B adds a transport and still needs A or C for the browser. C moves
the proof from the kernel to file permissions and adds a session to the web app, a bootstrap
page and an origin-guard change. Each of those is new security code.

## Open questions

Each has a safe default until the owner decides.

1. **`unsafe` in one module for Windows.** Candidate A on Windows needs four Win32 calls that no
   maintained crate wraps safely. Default: QUA-34 waits for the owner's answer, and Windows keeps
   refusing to start.
2. **The browser under WSL.** A Windows browser reaches a server in WSL 2 through WSL's
   localhost forwarding. Inside the Linux kernel the connection then comes from WSL's relay
   process, not from a process of the owner, and the Linux check would refuse it. Nobody has
   tried it. Default: under WSL the web app is supported only in a browser that runs inside
   WSL; the install stage (QUA-37) tries the Windows browser and records what happens.
3. **Stopping the server on Windows.** `quadratique server stop` sends SIGTERM on Unix. Windows
   has no SIGTERM. The command line refuses the stop on Windows (`cli/src/process.rs`).
   Default: QUA-34 adds a stop route that only the owner's command line can call, behind the
   caller check.
