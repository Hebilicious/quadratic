# Quadratic

Quadratic is a local issue tracker for one human and many AI agents that work in the same git
repository. Agents claim issues, record what they did and close them from their own worktrees.
The human follows the board in a web app, answers the issues that wait on a person, and reviews
the work. Quadratic watches the repository, so a merge moves the issue it names.

Quadratic is for a developer who runs several coding agents on one machine and wants one record
of who works on what, with no hosted service.

It has these parts:

- A server, written in Rust, that owns the store (doltlite) and listens only on `127.0.0.1`.
- The `quadratic` command line, and an MCP server for agents (`quadratic mcp`).
- A web app (Vue) that the server serves.
- A skill that `quadratic init` writes into a repository, so agents learn to use Quadratic.
- An install script with a systemd user unit.
- An evaluation harness that compares Quadratic with the trackers lific and Beads.

## Quick start

You need:

- Linux with a running systemd user manager (`systemctl --user status` answers).
- git, a C compiler and the zlib development files: the store engine is compiled from C. On
  Debian or Ubuntu: `sudo apt install git build-essential zlib1g-dev`.
- [proto](https://moonrepo.dev/proto), which installs the other tools at the versions that
  `.prototools` pins.

```sh
git clone https://github.com/Hebilicious/quadratic
cd quadratic
proto use                            # installs moon, Node.js, pnpm, Rust, Pkl, Java, Python and uv
pnpm install                         # installs the JavaScript dependencies
moon run quadratic-install:install   # builds the binary, installs it and starts the user service
```

The install puts the binary in `~/.local/bin/quadratic` (or `$XDG_BIN_HOME/quadratic`): that
folder must be on your `PATH`. Then, in the git repository where your agents work:

```sh
quadratic init --key ABC --name "My project"
quadratic ui --open
```

`init` writes the skill to `.agents/skills/quadratic/`, adds one line to `AGENTS.md` and creates
the project. `ui --open` opens the web app in your browser through `xdg-open`; without a desktop,
`quadratic ui` prints the address to open.

## Documentation

- [`apps/quadratic/README.md`](apps/quadratic/README.md): install, upgrade, uninstall, backups,
  security, agents in a sandbox, and how to develop and test Quadratic.
- [`apps/quadratic/eval/README.md`](apps/quadratic/eval/README.md): how Quadratic is evaluated,
  and a saved comparison with lific and Beads.

## Repository layout

The repository is a [moon](https://moonrepo.dev/moon) workspace with a pnpm workspace. The
product is in `apps/quadratic/`, in six moon projects: `quadratic-server`, `quadratic-cli`,
`quadratic-catalog`, `quadratic-web`, `quadratic-install` and `quadratic-eval`. Run every task
through moon from the root, for example `moon run quadratic-server:test`.

## License

MIT. See [`LICENSE`](LICENSE).
