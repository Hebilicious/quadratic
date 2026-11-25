# Quadratique — Product Requirements Document

Date: 2026-11-17 · Status: draft 1.

Quadratique is an issue tracker for **one person working with many AI agents**. It keeps every
task, claim and change in one local server, shows the work of all agents on one board, and
follows each task from the first idea through the merge to deployment and review.

## 1. Summary

Agents run in parallel, each in its own git worktree and branch. Quadratique tells each agent
what it can work on now, gives it the task atomically so no two agents take the same one,
records which agent, branch and worktree hold it, and keeps the full history of every
change. The human sees the whole picture in a fast, keyboard-first web app and is asked only
for decisions.

## 2. Goals and non-goals

### Goals
- **G1 — One board for parallel agents.** Every worktree and every agent reads and writes
  the same state, live.
- **G2 — Safe claims.** A claim is atomic; it records the agent, the git branch and the
  worktree.
- **G3 — Ready means ready.** Agents get only work whose blockers are closed.
- **G4 — Full history.** Every change has an author, a reason and an exact diff; the board
  at any past moment can be shown.
- **G5 — Beyond the merge.** A task continues after its branch is merged, through the
  statuses that the project's workflow names after `merged`, until it is done.
- **G6 — Clean agent integration.** One skill in the repository and one line in `AGENTS.md`
  that points to it; no hooks, no plugins, nothing written into agent configuration.
- **G7 — A UI a person enjoys.** List and board, keyboard first, live.

### Non-goals
- Team collaboration between several humans (roles, permissions, notifications).
- Orchestrating agents (starting, steering or scheduling them).
- Cycles, initiatives, capacity planning, SLAs.

## 3. Constraints

- Local-first: runs on the developer's machine, works offline.
- Backend in Rust.
- Storage: **doltlite** (a SQLite fork with Dolt's versioning: commits, diffs, blame, history
  queries), embedded in the server.
- Agent interface defined once as an **AcidCat** catalog
  (https://github.com/Hebilicious/acidcat), from which the CLI, the MCP server, HTTP and the
  page's WebMCP tools are generated.
- UI built with **heb-stack** (https://github.com/Hebilicious/heb-stack): Vue Vapor, Effect,
  XState, cssforge.
- Quadratique was first built inside another repository under the name Quadratic. On
  2026-11-25 it moved to its own repository, https://github.com/Hebilicious/quadratique, under
  the name Quadratique.

## 4. Users

| User | Needs |
|---|---|
| The human | One view of all work; only decisions reach them; fast triage by keyboard |
| Agents | The next ready task, an atomic claim, clear acceptance criteria, a short workflow text, JSON output |
| Other tools | Import of findings (e.g. a linter or a checker), events (deploys) that move issues on after the merge |

## 5. Prior art, and what Quadratique takes from it

| Tool | Take | Leave |
|---|---|---|
| **Linear** | Status categories with named statuses inside them; priority 0–4; float sort order; sub-issues; `blocks` relations; keyboard-first list and board; saved views; branch names containing the issue ID | Teams, cycles, initiatives, triage, IDs that change when an issue moves |
| **Beads** | A dependency graph where only some links block; "ready" as the main query; atomic claim; `discovered-from` links; one shared store for all worktrees | Gates on external events; hooks and plugins written into each agent's configuration; several install paths that must detect each other; no UI; unbounded scope |
| **Backlog.md** | "Ready" computed, never stored, failing closed; a short instruction pointer in `AGENTS.md`; a browser board | Task state stored per branch, invisible to other worktrees until a merge; body sections as markers agents can break |
| **lific** | One server owning one database; a web UI and MCP built in; an identity per agent; a "workable" filter; closing with evidence | No git awareness; history as an activity log only |

What Quadratique adds that none of them has:
1. **Git and worktrees:** claims record branch and worktree; merges are seen by watching the
   repository; commits link to issues through `Refs: ABC-12`.
2. **Life after the merge:** `merged` is a status, and the workflow can name more after it.
3. **Real history:** every change is a database commit with its diff and blame.
4. **Findings from other tools become issues automatically** (§10).
5. **The AcidCat interface and the heb-stack UI.**

## 6. Architecture

```
agents (CLI, MCP) ─┐
web UI (heb-stack) ├──▶ Quadratique server (Rust) ──▶ doltlite database (one file)
importers, events ─┘         │
                             └──▶ git repository (read-only: refs, commit messages)
```

- **One server owns the database.** The CLI, the MCP server and the web UI all talk to it;
  the CLI starts it when it is not running. Nothing else opens the database file.
- **One database commit per capability call that changes something.** The commit's author is the agent's identity;
  its message is the event (`issue.claim ABC-12`, agent, branch, worktree). Each change thus
  carries its meaning and its exact row diff.
- **Claims are atomic** because every agent writes to the main database branch through the
  one server, in one transaction. Database branches remain available for other uses (trying
  a bulk re-plan, then merging or dropping it).
- **Numbered IDs** (`ABC-12`) come from the server; they never change.
- **Live updates:** the server broadcasts each committed change (`issue.changed`) to the UI
  and to `watch` on the CLI.
- **Remote access:** the local listener answers only processes of the account that runs the
  server, and every administration capability is there (on macOS and Windows, see
  [decision 0003](decisions/0003-the-caller-check-on-macos-and-windows.md)). The rest is planned
  and not built: a browser on another device of the user's tailnet holds a session that the user
  approved on the machine that runs the server. An agent off the tailnet uses MCP over the
  internet with an agent token, through a backend that serves nothing else. Each ingress has its
  own backend. Agent tokens can be created and revoked, but no route accepts one yet.

## 7. Data model

- **Workspace → project → issue.** A project has a prefix (`ABC`).
- **Issue fields:** title; status; priority (0 none, 1 urgent … 4 low); labels; parent
  (sub-issues, any depth); milestone; sort order (a float); due date (optional);
  assignee (the human or an agent); claim (agent, branch, worktree, since).
- **Body as separate fields**, never markers in one text: description, acceptance criteria,
  notes, evidence. The acceptance criteria are required when an issue is created.
- **Relations:** `blocks`, `related`, `duplicate`, `discovered-from`, `stacked-on`. Only
  `blocks`, `stacked-on` and parent/child affect readiness (`stacked-on`: see
  [decision 0001](decisions/0001-stacked-on.md)).
- **Ready** is computed, never stored, and fails closed: an unclear blocker blocks.
- **Status categories:** backlog, unstarted, started, completed, canceled; each project names
  its statuses inside them.
- **Events** on every issue; every actor has an identity.

## 8. Lifecycle

Default workflow for a code project:

`backlog → todo → in progress → in review → merged → done`

- `merged` is in the started category: the work is in the default branch but not finished.
- A project whose work has steps after the merge (a deploy, a check) names them as statuses
  after `merged`, and a script, an agent or the human moves the issue on to done; a project
  with no such step is done at merge. Work to check after the merge is a new issue that the
  first one blocks, so it becomes ready when the first is done.
- **Merge detection** without git hooks: the server watches the repository's refs (polling or
  a file watch on `.git/refs`) and matches the `Refs: ABC-12` trailer in commit messages and
  the issue ID in branch names (`abc-12-short-title`).

## 9. Agent interface (AcidCat catalog, first draft)

| Capability | Read-only | What it does |
|---|---|---|
| `issue.list` | yes | Filtered list (status, label, project, assignee, ready) |
| `issue.ready` | yes | Unclaimed issues whose blockers are all closed, in order |
| `issue.get` | yes | One issue with relations and events |
| `issue.create` | no | Create; idempotent with a client key |
| `issue.update` | no | Change fields or status |
| `issue.claim` | no | Atomic; fails if held; records agent, branch, worktree. An MCP session that runs outside the worktree names it (`worktree`) and then acts from it. Planned, not built: a remote agent gives its branch, has no worktree, and its token is the holder |
| `issue.release` | no | Give a claim back |
| `issue.link` | no | Add or remove a relation |
| `issue.close` | no | Close with evidence (what was checked) |
| `findings.import` | no | Record a tool's run: each finding becomes an issue; a complete run finishes the issues whose finding is gone |
| `issue.advance` | no | On the machine that runs the server: move to a later status every issue that landed up to a commit of the default branch and waits after the merge (for a deploy script) |
| `history.query` | yes | Changes to an issue or the board, as of a time or commit, with authors |
| `workspace.instructions` | yes | The command reference for agents, and who the caller is |
| `token.create` | no | On the machine that runs the server: a token for one agent (a label), written only to a new file |
| `token.list` | yes | On the machine that runs the server: every token's label and dates, never its value |
| `token.revoke` | no | On the machine that runs the server: end a token |
| `session.approve` | no | Planned, not built. On the machine that runs the server: approve the tailnet browser session that shows a pairing code |
| `session.list` | yes | Planned, not built. On the machine that runs the server: every browser session's label, origin and dates |
| `session.revoke` | no | Planned, not built. On the machine that runs the server: end a browser session and its live streams |
| event `issue.changed` | – | Live stream for the UI and `watch` |

- **Discovery**, in three steps, each read only when it is needed: one line in a repository's
  `AGENTS.md` ("We use Quadratique for issues: every task is an issue there. Use the
  `quadratique` skill (`.agents/skills/quadratique/SKILL.md`) for anything about tasks."); the
  skill in the repository's skills folder, which says when to use Quadratique and how the work
  flows from a plan to a merge; and `quadratique instructions`, which prints the commands and who
  the caller is. Nothing else is written into agent configuration.
- The CLI derives the project, branch and worktree from the working directory
  (`git rev-parse --git-common-dir`, `--abbrev-ref HEAD`).
- Every command takes `--json`.

## 10. Findings importer

A tool that emits findings as JSON can feed Quadratique. The importer turns each finding into
an issue with structured links (for example fact IDs or files), and closes it when the next run
no longer reports it.

| Finding | Issue | Closes when |
|---|---|---|
| A finding | One issue per finding key, with the tool as its source | A complete run no longer reports it |
| A decision | A decision for the human, blocking every issue of the same tool that shares a `fact` link with it | The next run no longer reports it |
| A finding that comes back | Its issue returns to the first unstarted status | As above |
| A finding the human overruled | Its canceled issue stays canceled | – |

Effects: agents never work over an undecided fact (it blocks them); the human sees only
decisions; a change that a checker verifies is `merged` when it lands and done when the checker
reports nothing for it.

## 11. Web UI

- List and board over the same order; group by status, priority, label, project, milestone
  or parent; saved views (filters only).
- Keyboard first: `C` create, `J/K` move, `X` select, `Cmd K` command menu, `Space` peek,
  `Cmd B` list/board, `G I` / `G M` go to.
- Live updates; every card shows its claim (agent, branch, worktree).
- A **now** view: what every agent is doing, and what waits on the human.
- A **history** view: the board as it was at any moment; who changed what.

## 12. Evaluation: hill-climbing against lific and Beads

Quadratique, lific and Beads run the same scripted scenarios, and Quadratique is improved until
it wins or ties on each measure, except two that were accepted after the first evaluation: one
setup step more than Beads (since then tied), and a whole MCP tool list larger than lific's. A
change is kept only if the scores improve. The harness and the latest results are in
[`apps/quadratique/eval/README.md`](../apps/quadratique/eval/README.md).

- **Scenarios:** several agents in parallel worktrees pick ready work, claim, hit a blocker,
  discover a new issue, finish, merge, move the issue on after a deploy; the human triages from the UI; a checker
  produces findings.
- **Measures:**
  - tasks completed without human help;
  - double claims and lost updates (target: zero);
  - tokens agents spend on the tracker (instructions, tool schemas, calls);
  - setup steps and files written into the repository;
  - time for the human to answer "what is every agent doing, and what waits on me?";
  - history questions answered correctly ("who changed this, when, on which branch?").

## 13. Milestones

| M | Content | Exit |
|---|---|---|
| M0 | The AcidCat catalog and the event schema | Approved |
| M1 | Server, doltlite store, CLI | Create, claim, ready, close and history from the CLI; tests prove claims are atomic under concurrent agents |
| M2 | MCP server, `instructions`, the skill, the `AGENTS.md` line in a first repository | An agent in a worktree takes a ready issue and closes it with evidence |
| M3 | Web UI: list, board, issue page, keys, live updates, now view | A real backlog is run from the UI for a week |
| M4 | Git awareness: ref watcher, `Refs:` trailers, branch names, `merged` | Merging a branch moves its issue to `merged` with no manual step |
| M5 | The findings importer | Checker findings open and close issues |
| M6 | Evaluation harness against lific and Beads | Quadratique wins or ties on every measure but two, which were accepted: one setup step more than Beads (since then tied), and a whole MCP tool list larger than lific's |
| M7 | Remote access, in parts that can each be the last. Part 0: spikes of the ingress and the claude.ai connector. Part 1, private: a local listener for the server's own account only (the kernel names the caller's account; no secret), agent tokens, the board on a tailnet browser with an approved session. Part 2: MCP on the internet for a cloud agent, with an agent token, only after Part 0 and an explicit go-ahead. Part 3: a move by commit, `issue.advance`, a local command: "every issue that landed up to this commit moves to this status". Built: Part 1's local listener and agent tokens, and Part 3. Not built: the tailnet browser board, Part 0 and Part 2 | A phone and a cloud agent use the same board; a deploy script moves the issues of the commit it deployed |
| M8 | Quadratique in T3 Code (proposal): a tracker for T3's issues feature that reads and writes through the `web` target as the human; threads linked to the issues their agents claim | In T3 on a phone, the human lists a project's issues, opens one with its notes, moves it, and sees the thread that claimed it with no manual link |

## 14. Success metrics

- Zero double claims and zero lost updates in the evaluation scenarios.
- Fewer tokens per completed task than lific and Beads.
- No file written into a repository except the skill and the one `AGENTS.md` line.
- The human answers "what waits on me?" in one screen.

## 15. Risks

| Risk | Mitigation |
|---|---|
| doltlite is young | Backups of the single file; its SQLite API keeps a path back to plain SQLite |
| The server is not running | The CLI starts it; a user service keeps it up |
| Agents ignore the tracker | `instructions` is short; `issue.ready` is the easiest way to find work |
| Scope creep toward orchestration | Non-goals; the evaluation measures only tracking |
