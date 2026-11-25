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

**What identifies the caller.** A local address and port alone do not identify a TCP
connection: the same local port can appear in a listener's row, in rows of other states, and on
another address. The server knows both endpoints of the connection it accepted: its own address
and port (`Connection::local`) and the client's (`Connection::peer`). It looks for the client's
socket, which is the same connection seen from the other side: a connected TCP socket in the
ESTABLISHED state whose local endpoint is the server's `peer` and whose remote endpoint is the
server's `local`, both address and port. The kernel allows only one connected socket per
four-tuple, so at most one socket matches. No match, a listener row, a row in another state, or
two different sockets that match refuse the connection. Each platform's lookup is held to this
rule. The listener binds only to `127.0.0.1` (`server::start_with` refuses any other address),
so the lookup reads IPv4 data only.

- **macOS.** No system call maps a TCP connection to a uid. `libproc` lists the sockets of a
  process: `proc_listpids` with `PROC_UID_ONLY` and the owner's uid lists the owner's processes;
  `proc_pidinfo(PROC_PIDLISTFDS)` and `proc_pidfdinfo(PROC_PIDFDSOCKETINFO)` give each socket's
  state and both endpoints (`tcpsi_state`, and `insi_laddr`, `insi_lport`, `insi_faddr`,
  `insi_fport` of `tcpsi_ini`). This is what `lsof` does. The server admits the connection when
  a socket of one of the owner's processes matches both endpoints as above and is ESTABLISHED.
  Descriptors of one socket that `fork` shares between several of the owner's processes count
  as one socket. macOS answers `proc_pidfdinfo` only for processes of the same uid, unless the
  caller is root. So a socket of another account is never found, and the connection is refused.
  That is the fail-closed property, but it rests on the kernel's access policy. QUA-33 must
  prove it with a test in which a second account connects.
  - Cost: one scan of the owner's processes and their descriptors for each new connection. A
    few hundred processes take some milliseconds. HTTP keep-alive spreads the cost, and the
    command line opens one connection per command. QUA-33 measures it.
  - Safe interface: the `libproc` crate wraps these calls. Its own socket structure gives the
    TCP fields only through unions, which need `unsafe` to read; QUA-33 reads them from plain
    bytes instead (see "macOS, as built" below).
  - Gap: a process of the owner may hand its socket to a process of another account (through
    `SCM_RIGHTS`). The owner chose to do that. Linux has the same gap, because the socket's uid
    stays the creator's.
- **Windows.** `GetExtendedTcpTable` lists every TCP endpoint with no privilege. Its IPv4 and
  IPv6 tables are separate (`ulAf` is `AF_INET` or `AF_INET6`); the server asks for the IPv4
  table only, because its listener is IPv4 only. It asks for connections only
  (`TCP_TABLE_OWNER_MODULE_CONNECTIONS`, so no listener rows) and applies the rule above:
  exactly one row in `MIB_TCP_STATE_ESTAB` whose local endpoint is the server's `peer` and whose
  remote endpoint is the server's `local`. That row (`MIB_TCPROW_OWNER_MODULE`) names a process
  and a moment, and neither is the owner of the socket the way Linux records it:
  - `dwOwningPid` is "the PID of the process that issued a context bind for this TCP
    connection". Winsock lets that process share the socket with any other process through
    `WSADuplicateSocket`, and the socket stays open until its last descriptor closes. The row
    then still names the process that bound it, which may have exited.
  - A process id can be reused. Between the table read and the token query, the bound process
    can exit and its id can go to another process, which may belong to the owner. The earlier
    version of this record argued that only a child of the same account could hold such a
    socket; that does not hold, because the socket can be handed to any process, so that
    argument does not make the pid-to-token lookup safe.

  So the Windows lookup reads the token of a process instance that it ties to the row, in this
  order: read the row; open the process with `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)`
  and hold the handle, which keeps that instance's id from being reused while it is held; read
  the instance's start time with `GetProcessTimes` and refuse when it started after the row's
  `liCreateTimestamp` (the moment of the bind: a later process cannot have bound it, so the id
  was reused); read the token's user with `OpenProcessToken` and `GetTokenInformation(TokenUser)`
  and compare it with the server's own SID; read the table again and refuse unless the same row
  is still there with the same pid and timestamp; then close the handle. Any failure refuses,
  including `OpenProcessToken` on another account's process, which Windows refuses to a
  non-elevated caller.
  - What is proven: the documented fields and their meaning (sources below). What is not proven
    and must be shown by tests before QUA-34 lets Windows serve: that a second account's
    connection is refused; that a row whose binding process exited is refused (by the start
    time, or because the row goes away), including after its id is reused; that the row of a
    socket shared through `WSADuplicateSocket` names the process that bound it, so that a
    socket bound by another account and handed to the owner's process is refused, and a socket
    bound by the owner and handed to another account is the owner's choice as on Linux.
  - Until then the server keeps refusing to start on Windows.
  - Safe interface: no maintained crate gives these calls safely. The `windows` crate declares
    them `unsafe`. So QUA-34 needs one module that may use `unsafe` (the workspace's `forbid`
    becomes `deny`, and the module says `allow` with a comment for each call). That is an
    owner's decision. The open questions below list it.
  - Sources: [GetExtendedTcpTable](https://learn.microsoft.com/en-us/windows/win32/api/iphlpapi/nf-iphlpapi-getextendedtcptable),
    [MIB_TCPROW_OWNER_PID](https://learn.microsoft.com/en-us/windows/win32/api/tcpmib/ns-tcpmib-mib_tcprow_owner_pid),
    [MIB_TCPROW_OWNER_MODULE](https://learn.microsoft.com/en-us/windows/win32/api/tcpmib/ns-tcpmib-mib_tcprow_owner_module),
    [WSADuplicateSocket](https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-wsaduplicatesocketw).
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
| A, macOS | the kernel's rule that only the same uid may read a process's sockets | unchanged | a lookup through `libproc`, matching both endpoints | no (`libproc` crate) |
| A, Windows | the kernel's rule on another account's process token, tied to the row by a held handle, the start time and a second read | unchanged | a lookup through six Win32 calls, matching both endpoints; tests before serving | one module |
| B | the socket's or the pipe's permissions, checked by the kernel | needs A or C as well | a second transport, everywhere | Windows pipe security: one module |
| C | the file's permissions | a bootstrap page, a new session, an origin-guard change | the secret, the session, the Windows ACL | Windows ACL: one module |

## Decision

1. **Candidate A on both platforms.** The server keeps one TCP listener and asks the system who
   owns the other end of each connection: `libproc` on macOS (QUA-33),
   `GetExtendedTcpTable` and the process token on Windows (QUA-34). Each lookup matches exactly
   one connected socket by both endpoints and fails closed. On Windows the token is read from a
   process instance tied to the row as candidate A says, and Windows serves only after the tests
   listed there pass. As on Linux, the server looks up a connection of its own before it serves,
   and refuses to start if the lookup does not name its own account.
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

## macOS, as built (QUA-33)

Added 2026-11-25. The code is `server/src/peer_macos.rs` (the calls) and
`server/src/socket_fdinfo.rs` (the bytes and the matching rule, tested on every platform).

- **Tests per platform** (server and command line, passed): macOS 184 before (no test started a
  server), 463 after; Linux 461 before, 467 after; Windows 180 before, 186 after. The new tests on
  every platform are the 6 tests of `socket_fdinfo.rs`. "Web journeys" is required on macOS.
- **Calls.** `libproc` 0.15.0: `pids_by_type(ProcFilter::ByUID)` (`proc_listpids` with
  `PROC_UID_ONLY` and the server's effective uid, which is the uid the kernel's same-user rule
  compares), `pidinfo::<BSDInfo>` and `listpidinfo::<ListFDs>` (`PROC_PIDLISTFDS`), and
  `pidfdinfo` (`PROC_PIDFDSOCKETINFO`) for each socket descriptor. After the scan, the server
  reads the uid (`pbi_uid`) of each process that holds the socket again. It admits the
  connection when one of them still runs as the owner. A process id that another account
  reused between the scan and that read names that account, and the connection is refused.
- **No `unsafe` in the workspace.** `pidfdinfo` takes any type that implements the safe trait
  `PIDFDInfo`. The server gives it a `[u8; 792]` buffer (`sizeof(struct socket_fdinfo)`) and
  reads the fields it needs from fixed offsets, as the Linux check reads netlink bytes. Every
  byte pattern is a valid value, so a wrong layout can make the lookup find nothing, but it
  cannot cause undefined behaviour. On macOS the build checks each offset against the crate's
  `repr(C)` declaration of the structure (`const` assertions with `offset_of!`). The server's
  self-check at start checks them against the kernel. The kernel refuses a buffer that is
  smaller than its structure, so if a later macOS makes the structure larger, every lookup
  finds nothing and the server does not start.
- **Rule.** As above: exactly one socket that is TCP, IPv4 (`INI_IPV4`) and `ESTABLISHED`,
  whose local end is the server's `peer` and whose remote end is the server's `local`.
  Descriptors that share one socket (the same `soi_so`) count as one socket. No match, another
  state, or two sockets refuse. A process that exits during the scan, or whose descriptors the
  kernel does not show, holds no socket for the scan. A failure to list the processes refuses
  the connection, and the server reports it once in its log.
- **Self-check.** Before it serves, the server connects to a listener of its own and looks up
  that connection. When the lookup fails, the server does not start. The message says that the
  server must start from a normal session of the account, outside any sandbox, and where to
  report the macOS version.
- **Dependencies.** `libproc` and, at build time, `bindgen` with `clang-sys` (9 crates in the
  lock file). They are used on macOS only: `bindgen` needs libclang, which the Xcode command
  line tools provide. Those tools are also needed to compile the C store engine.

What CI proved on macos-latest ([PR 28](https://github.com/Hebilicious/quadratique/pull/28), [run 45555375519](https://github.com/Hebilicious/quadratique/actions/runs/45555375519)):

- The owner's connection is admitted, and a scan of every process finds its holder: `nc` of the
  owner, `Holders([pid])`.
- Another account is refused. A process of the account `nobody` holds a connection (`sudo -n -u
  nobody nc`). Its connection is refused. A scan of every process of the machine (not only the
  owner's) finds nothing. The kernel lists nobody's processes, but shows this account none of
  their TCP sockets. Through the whole server, `sudo -n -u nobody curl` gets `403` and the
  owner-only answer (`test-as-nobody`). The test uses the existing account `nobody` and creates
  no account. It runs only on the CI runner, where `sudo` needs no password.
- A connection that closed before the lookup, a peer port with no socket, a port that only a
  listener holds, a failing lookup, and a self-check that finds nothing, another uid, or an
  error are refused (unit tests in `peer_macos.rs`).
- The test that admits 1000 of the owner's connections while other sockets of the same process
  open and close (`process::the_owner_is_admitted_while_other_sockets_open_and_close`) passes on
  macOS too.
- Cost, measured on the runner (`the_cost_of_the_caller_check`, 200 of each): one lookup over the
  300 processes of the runner's account takes 0.63 ms at the median, 0.9 ms at the 95th
  percentile, and 6 ms at most. A request to the server on a new connection takes 0.84 ms at the
  median (1.3 ms at p95). On one kept-alive connection it takes 0.17 ms, because the check runs
  once for each connection. A browser opens a few kept-alive connections and makes its many
  requests over them, so the check adds less than a millisecond for each new connection. The 14
  web journeys pass on macOS in 39 s (1.3 min on the Linux runner). The cost grows with the
  number of the owner's processes and their descriptors, not with the number of requests.
- In the first CI run, two of the owner's own requests were refused, both in tests that run git.
  Between `fork` and `exec`, a child process holds a copy of every descriptor of its parent,
  the client's socket included. `proc_listpids` lists the newest process first, so the lookup
  took the child as the holder. When it read the holder's uid again, the child had already
  exec'd git or ended, and the connection was refused. The lookup now keeps every holder of the
  socket and admits when any of them still runs as the owner (`account_of_holders`, tested on
  every platform). The code shows this path; one CI run after the fix passed, which alone does
  not prove that no other path refuses the owner. `pbi_nfiles` is the size of the descriptor
  table (150, with 116 descriptors open and the highest at 146), so a buffer of that size holds
  every descriptor. The lookup still asks `PROC_PIDLISTFDS` again with more room when the answer
  fills the buffer, because the table can grow during the scan.

Gaps that remain on macOS:

- The check names the account of a process that holds the socket now, not the account that
  created it. A socket that the owner's process hands to a process of another account
  (`SCM_RIGHTS`) is the owner's choice, as on Linux. A process that holds the owner's socket and
  then changes its effective uid is no longer listed, so its connection is refused.
- A server that runs as root may read every process's descriptors. It still lists only root's
  processes, and it reads the holder's uid again after the scan.

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
3. **WSL and Windows reaching each other's server.** The second question covers only a Windows
   browser reaching a server in WSL. The other direction, a process in WSL reaching a server on
   Windows, uses the host's address in WSL 2's default NAT mode, which is outside the server's
   loopback bind, so it does not reach the server at all. Mirrored mode gives `localhost` in
   both directions, but nothing says which identity the receiving kernel can attest for a
   connection that crosses from one system to the other
   ([WSL networking](https://learn.microsoft.com/en-us/windows/wsl/networking)). Default: in
   both directions a connection whose account the receiving kernel cannot attest is refused;
   QUA-37 and QUA-34 verify what each kernel reports before support is promised.
4. **Stopping the server on Windows.** `quadratique server stop` sends SIGTERM on Unix. Windows
   has no SIGTERM. The command line refuses the stop on Windows (`cli/src/process.rs`).
   Default: QUA-34 adds a stop route that only the owner's command line can call, behind the
   caller check.
5. **The `unsafe` code inside `libproc`.** The workspace has no `unsafe` code, but the macOS
   check depends on the crate's `unsafe` code. That code calls the C functions and writes the
   kernel's answer into a buffer of the type that the server gives. The server gives a byte
   array, so any answer is a valid value. The crate's `listpidinfo` sets the length of a
   vector before the kernel fills it (`set_len` on memory that is not initialised). For the
   integer items that the server reads, nothing in practice goes wrong, but the Rust rules do
   not allow it. Default: use the crate, pinned at `=0.15.0`, and read every new version's
   changes to these calls before an upgrade. The alternative is the one `unsafe` module that
   open question 1 asks about for Windows.
