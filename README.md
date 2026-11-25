# Quadratique

Quadratique is a local issue tracker for one human and many AI agents that work in the same git
repository. Agents claim issues, record what they did and close them from their own worktrees.
The human follows the board in a web app, answers the issues that wait on a person, and reviews
the work. Quadratique watches the repository, so a merge moves the issue it names.

Quadratique is for a developer who runs several coding agents on one machine and wants one record
of who works on what, with no hosted service.

It has these parts:

- A server, written in Rust, that owns the store (doltlite) and listens only on `127.0.0.1`.
- The `quadratique` command line, and an MCP server for agents (`quadratique mcp`).
- A web app (Vue) that the server serves.
- A skill that `quadratique init` writes into a repository, so agents learn to use Quadratique.
- An install script with a systemd user unit.
- An evaluation harness that compares Quadratique with the trackers lific and Beads.

## Quick start

You need:

- Linux with a running systemd user manager (`systemctl --user status` answers). On macOS the
  server and the command line build and run: after `proto use` and `pnpm install`, `moon run
  quadratique-web:build`, then `sh apps/quadratique/install/build.sh apps/quadratique/web/dist
  target` prints the path of the binary (it needs the Xcode command line tools). The install
  script below is Linux-only until QUA-37.
  Windows is not supported yet (QUA-34).
- git, a C compiler and the zlib development files: the store engine is compiled from C. On
  Debian or Ubuntu: `sudo apt install git build-essential zlib1g-dev`.
- [proto](https://moonrepo.dev/proto), which installs the other tools at the versions that
  `.prototools` pins.

```sh
git clone https://github.com/Hebilicious/quadratique
cd quadratique
proto use                            # installs moon, Node.js, pnpm, Rust, Pkl, Java, Python and uv
pnpm install                         # installs the JavaScript dependencies
moon run quadratique-install:install   # builds the binary, installs it and starts the user service
```

To install a prebuilt binary instead, with no toolchain, see "Install from a release" in
[`apps/quadratique/README.md`](apps/quadratique/README.md).

The install puts the binary in `~/.local/bin/quadratique` (or `$XDG_BIN_HOME/quadratique`): that
folder must be on your `PATH`. Then, in the git repository where your agents work:

```sh
quadratique init --key ABC --name "My project"
quadratique ui --open
```

`init` writes the skill to `.agents/skills/quadratique/`, adds one line to `AGENTS.md` and creates
the project. `ui --open` opens the web app in your browser through `xdg-open`; without a desktop,
`quadratique ui` prints the address to open.

## Documentation

- [`apps/quadratique/README.md`](apps/quadratique/README.md): install, upgrade, uninstall, backups,
  security, agents in a sandbox, and how to develop and test Quadratique.
- [`apps/quadratique/eval/README.md`](apps/quadratique/eval/README.md): how Quadratique is evaluated,
  and a saved comparison with lific and Beads.

## Repository layout

The repository is a [moon](https://moonrepo.dev/moon) workspace with a pnpm workspace. The
product is in `apps/quadratique/`, in six moon projects: `quadratique-server`, `quadratique-cli`,
`quadratique-catalog`, `quadratique-web`, `quadratique-install` and `quadratique-eval`. Run every task
through moon from the root, for example `moon run quadratique-server:test`.

## License

MIT. See [`LICENSE`](LICENSE).
