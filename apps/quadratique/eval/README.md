# Evaluating Quadratique

Quadratique is evaluated in two ways:

- **End-to-end runs.** Real agents use Quadratique on small test repositories, and a person (or a
  lead agent in that role) acts as the maintainer. A run shows whether the system works from the
  one line in `AGENTS.md` to merged work, and what gets in an agent's way.
- **The comparison harness.** [`harness/`](harness/README.md) runs the same scripted scenarios
  and the same agent sessions on Quadratique and on two other trackers, lific and Beads, and writes
  one table with a row per measure.

## Before you start

- The Rust toolchain, to build `quadratique` (see "Before you start" in the
  [Quadratique README](../README.md)).
- `git`, `bash` and `jq`: the seed scripts read the id of each issue they create with `jq`.
- For the comparison harness: Linux on x86-64, because the pinned releases are
  `lific-linux-x86_64` and `beads_1.3.4_linux_amd64`; Python 3.14 and uv (the harness's
  `pyproject.toml` asks for Python `>=3.14,<3.15`, and uv uses only a Python of the system); and
  network access to GitHub, to download those releases, and to the Python package index, from
  which `uv tool run` fetches the `beads-mcp` package for the MCP rows.
- `git`, `python3`, `uv` and, for the agent sessions, a signed-in `claude`, on the `PATH` that
  starts the harness. Each tracker command and each session runs with a narrow `PATH`: the
  wrappers of the trackers, the folders where those four commands were found, then
  `/usr/local/bin`, `/usr/bin` and `/bin`. A missing `uv` stops the MCP rows with a message that
  names it.

## Run an end-to-end evaluation

Build the binary and seed the test repositories. Run the seed scripts as the human, outside any
agent session, so that the issues are created by the human. From `apps/quadratique` (the Rust
workspace, the folder with `Cargo.lock`):

```sh
cargo build --release -p quadratique --target-dir target/qeval   # not target/release: the install builds there
scratch="$(mktemp -d)"                    # a folder outside every repository
export QUADRATIQUE_HOME="$scratch/home"     # never your real data folder
export PATH="$PWD/target/qeval/release:$PATH"
env -u CLAUDECODE -u AI_AGENT bash eval/seed-tally.sh "$scratch/tally"
env -u CLAUDECODE -u AI_AGENT bash eval/seed-pomo.sh "$scratch/pomo"
git -C "$scratch/tally" worktree add -b agent-a "$scratch/tally-wt-a" main   # one per agent
```

Stop the server of that data folder when the run is over: `quadratique server stop`, in the same
shell.

The seed scripts create a repository and its Quadratique project as the human would:

| Script | Repository | Project | Issues |
|---|---|---|---|
| `seed-tally.sh` | `tally`, a Python word counter | `TAL` | Eight: three ready, three blocked by another issue, one decision assigned to the human, one assigned to the role `docs` |
| `seed-pomo.sh` | `pomo`, a shell work timer | `POM` | Two, both ready. A second project in the same workspace shows that a repository sees only its own work |
| `seed-review.sh` | `tally`, with Quadratique's skill and an `AGENTS.md` that asks for the maintainer's review before a merge | `TAL` | None: the run starts from a conversation |

With `seed-tally.sh` and `seed-pomo.sh`, each agent gets a worktree and this prompt, and nothing
else about Quadratique:

> Read its `AGENTS.md` and follow it. Do one task from start to finish, then stop.

The maintainer then reviews each branch, merges it, calls `quadratique finish` (the issue becomes
`merged`), moves it to `done` when the checks on `main` pass, answers the issues assigned to the
human, and starts the next wave on what unblocked.

With `seed-review.sh`, the maintainer talks to two sessions and writes only what a person would
write, nothing about how to use Quadratique: a planner that writes a PRD with the maintainer and
turns it into issues, then an implementer that takes the issues one at a time and asks for review
before each merge.

## Run the comparison

From the root of the repository:

```sh
moon run quadratique-eval:test                                  # the harness's own tests
moon run quadratique-eval:scripted -- --rounds 50 --repeats 3   # no model; prints the run folder
run="<the run folder that scripted printed>"
moon run quadratique-eval:check-home                            # must pass before the paid sessions
moon run quadratique-eval:agents -- --run "$run"                # nine paid Claude Code sessions
moon run quadratique-eval:report -- --run "$run"                # results.json -> results.md
```

The run folders are in `~/.cache/quadratique-eval/runs/`, or in the folder that `QEVAL_RUNS` names.
`scripted` and `agents` write their measures to `results.json` in the run folder; they and
`report` write the table to `results.md` in it and print it. To combine runs, for
example the agent sessions of one run with the scripted measures of another, pass `--from` once
per run and `--out` for the folder of `results.md`:

```sh
moon run quadratique-eval:report -- \
  --from "agents=$HOME/.cache/quadratique-eval/runs/first" \
  --from "scripted=$HOME/.cache/quadratique-eval/runs/second:quadratique,lific" \
  --out "$HOME/.cache/quadratique-eval/runs/combined"
```

Each `--from` is `agents=`, `scripted=` or `all=`, then a run folder, then optionally `:` and the
trackers to take from it. `report` with neither `--run` nor `--from` and `--out` says what to
pass and exits with status 1. [`harness/README.md`](harness/README.md) describes the setup of
each tracker, the fairness rules and the safety limits.

### How a row is called won, tied or lost

One rule applies to every row: **no overlap between the runs.** A cell holds one value per run of
that tracker: per session for the agent rows that are per session, per repeat for the races, per
run folder otherwise, and a range (lower, upper) where the measure has bounds. Quadratique **won** a
row when every Quadratique value is better than every value of each other tracker. It **lost** to a
tracker when every value of that tracker is better than every Quadratique value. Otherwise the row
is **tied**, whatever the medians say. A tracker with no value in a row ("not possible", "not
established") is left out of the comparison, and the label says so.

`qeval report` applies the rule (`harness/qeval/report.py`).

## Saved results

The table below is a saved result: its run folders are not part of this repository, so you
cannot produce it again from here. Run the comparison to produce your own table.

It was measured under these conditions:

- lific v2.11.1 and Beads v1.3.4, the releases that the harness pins.
- Quadratique built from this source as it was when the table was saved. A later change to the
  skill or to the catalog changes the token rows of a new run.
- The run predates the rename of Quadratic to Quadratique: the command was `quadratic`, and the
  longer name adds a few tokens per call. Counted again after the rename, the whole MCP tool list
  is 6,737 tokens (3 more) and the skill's `SKILL.md` 946 (1 more).
- Every session ran Claude Opus 5.5 at medium effort, with the prompt above.
- Every row comes from one run: three sessions for each tracker, and the scripted measures at
  50 rounds × 3 repeats. `report --run` wrote the table; each cell lists each session's or each
  repeat's value.
- The "Setup steps" row was counted again after a commit stopped counting as a step, from the
  saved run's step lists: they ended with the maintainer's commit for Quadratique and lific,
  which the count now leaves out (`setup_steps` in `harness/qeval/report.py`).

| Row | Quadratique | lific | Beads | Quadratique's result |
|---|---|---|---|---|
| Tasks completed (one value per run) | 3 (3 of 3 sessions) | 1 (1 of 3 sessions) | 2 (3 of 3 sessions) | won |
| Double claims, default identities (per repeat of 50) | 0; 0; 0 | 50; 50; 50 | 50; 50; 50 | won |
| Double claims, one identity per agent (second row) | 0; 0; 0 | 50; 50; 50 | 0; 0; 0 | tied with Beads |
| Double claims, lific passing `expected_seq` (second row) | – | 0; 0; 0 | – | – |
| Lost updates, two appends (per repeat of 50 writes) | 0 (25 refused); 0 (25 refused); 0 (25 refused) | 0; 0; 0 | 0; 0; 0 | tied |
| Lost updates, two read-modify-writes | 0 (25 refused); 0 (25 refused); 0 (25 refused) | 25; 25; 25 | 11; 14; 12 | won |
| Lost updates, read-modify-write, lific passing `expected_seq` (second row) | – | 0 (25 refused); 0 (25 refused); 0 (25 refused) | – | – |
| Double claims in the agent runs | 0 (transcripts: 0) | not established (transcripts: 2) | not established (transcripts: 1) | not compared |
| Lost updates in the agent runs | 0 of 3 checked, 3 made | 0 of 3 checked, 3 made | 1 of 2 checked, 3 made | tied with lific |
| Tracker instructions that reached each session | 1,023; 1,023; 1,023 | 1,832; 1,832; 1,832 | 3,109; 3,109; 3,109 | won |
| Tracker text at the first request (second row) | 39; 39; 39 | 698; 698; 698 | 2,159; 2,159; 2,159 | won |
| First request's billed input (second row) | 8,127; 8,127; 8,127 | 9,053; 9,053; 9,053 | 11,262; 11,262; 11,262 | won |
| Whole MCP tool list (second row) | 6,734 (11 tools) | 5,783 (31 tools) | 13,522 (18 tools) | lost to lific |
| Tracker tokens per distinct task (one range per run) | 2,371–2,702 | 7,709 | 5,002–7,404 | won |
| Tracker tokens per session (second row) | 2,350–2,643; 2,296–2,624; 2,466–2,839 | 2,278; 2,720; 2,711 | 3,258–4,681; 3,288–4,970; 3,457–5,158 | tied with lific |
| Setup steps | 1 | 4 | 1 | tied with Beads |
| Files written into the repository | 2 | 2 | 18 | tied with lific |
| Lines added to `.git/config` | 0 | 0 | 3 | tied with lific |
| Files written outside the repository (second row) | 6 | 7 | 1 | – |
| The human's question on the command line: facts and steps (a proxy for the human's time) | 4 of 4 facts, 2 commands | 1 of 3 facts, 3 commands (not possible: t8 waits on the human (review)) | 1 of 3 facts, 2 commands (not possible: t8 waits on the human (review)) | won |
| History questions, default identities | 10 right, 0 wrong, 0 not possible (2 commands) | 5 right, 3 wrong, 2 not possible (11 commands) | 5 right, 3 wrong, 2 not possible (12 commands) | won |
| History questions, one identity per agent (second row) | 10 right, 0 wrong, 0 not possible (2 commands) | 8 right, 0 wrong, 2 not possible (11 commands) | 8 right, 0 wrong, 2 not possible (12 commands) | won |
| A real merge, shown with no tracker command | right: `merged` (1 read; `in-review` before the merge) | not possible: `done` (1 read; `done` before the merge) | not possible: `closed` (1 read; `closed` before the merge) | not compared |
| Moving the issue on after a deploy | 2 commands to `done` | not possible: shows `done` since the agent's finish | not possible: shows `closed` since the agent's finish | not compared |
| A checker's run, then one finding gone | right: 1 and 1 commands, through its import | workaround: 3 and 2 commands, through a script around its own commands | workaround: 3 and 2 commands, through a script around its own commands | not compared |

### Where Quadratique loses or ties

| Row | Why | What would move it |
|---|---|---|
| Whole MCP tool list (6,734 against 5,783) | Each tool bundles the types it uses, so the types of an issue repeat in `issue.list`, `issue.create` and `issue.update`, which are the largest tools. The `Date` and `Time` types, whose patterns check real dates, repeat in four tools | A shorter `Date` or `Time` pattern changes what the server accepts. A session fetches only the definitions of the tools it uses: the row of the tracker instructions that reached each session is won |
| Tracker tokens per session (tied with lific) | The skill is the largest piece, then `claim` and `close`, which answered the whole issue in this run. A refused claim answers the holder's claim with its worktree path; sessions that tried a held issue first paid that once or twice | A shorter skill; or answers that leave out empty arrays and the claim's worktree for the agent's route, which changes catalog types that the web app reads. More sessions per tracker would show whether the overlap is real |

### What each row measures

- **Tasks completed.** A session completes at most one task. It counts when the session's
  transcript has a successful finishing call on a seeded issue, the tracker shows that issue
  finished when the harness checks after the session (it does not check which actor finished it),
  the agent's branch changes a file that task is about, and `python3 -m unittest` passes on that branch with at least one test run
  and none skipped that the task was to enable. A task whose issue asks for tests also needs a
  test file changed on the branch: a `.py` file under `tests/` that the branch adds or modifies.
  The number counts distinct issues: when several sessions finish the same issue, it counts once.
- **Double claims.** Two processes in two worktrees take a new issue, released by one barrier. A
  double claim is a round in which both are told they have it. lific has no claim: setting an
  issue `active` succeeds for both unless they pass `expected_seq`, which its instructions do not
  mention. Beads' second claim succeeds when both agents have the same identity, which is the
  default. In the agent runs, double claims come from each tracker's own record of who took what
  and when. Beads' and lific's records name every agent alike, so they cannot tell the sessions
  apart, and the row reports what the transcripts show instead.
- **Lost updates.** A write that the tracker reported as done and whose text is absent
  afterwards. Quadratique loses nothing because every update names the revision it read: the
  second of two writes at the same instant gets `Stale`, for appends too, where lific and Beads
  accept both. In the agent runs, a write counts as present only when its text is found, as whole
  words, in one written field of its issue.
- **Tracker instructions that reached each session.** Read from each transcript: the tracker
  text in context at the first request, every read of the tracker's instruction text, every
  ToolSearch fetch of its MCP tool definitions, and every run of its instructions command.
- **Tracker text at the first request.** Claude Code loads `CLAUDE.md`, and loads `AGENTS.md`
  only where the project has no `CLAUDE.md`. So Quadratique and lific sessions start with
  `AGENTS.md`, and Beads sessions start with the `CLAUDE.md` that `bd init` writes.
- **First request's billed input.** Claude's own count from the first request's usage (input,
  cache creation and cache read). It checks the row above without the harness's classification.
- **Tracker tokens.** Every piece of the instructions row, plus every tracker call's input and
  output, each counted once, when it entered the context. Every token row of the agent runs is a
  lower and an upper bound: the upper bound counts every call that touches the tracker whole, the
  lower bound only what is certainly the tracker's. No number is exact.
- **Setup steps and files.** The commands of the quickstart until an agent can create an issue:
  `quadratique init`; lific's `init`, `bind`, `connect` and `agents-md`; `bd init`. A commit is
  not a step. Who commits is still a difference: `bd init` commits what it wrote, while
  `quadratique init` and lific leave the commit to the person, and the maintainer of the run made
  it. The files of one skill folder count as one file when its `SKILL.md` is among them. Beads'
  one step writes 19 files, three lines of `.git/config`
  (`core.hooksPath` and a `[beads]` section) and `~/.dolt/config_global.json` in the throwaway
  home. lific's setup commit puts the plaintext key of `.mcp.json` into git.
- **The human's question.** "What is every agent doing, and what waits on me?" Four facts must
  appear: agent-a works on `t1`, agent-b on `t7`, `t4` waits on the human, and `t8`, finished on
  agent-c's branch, waits for review. With default identities lific shows every agent as "Claude
  Code" and Beads as "maintainer", so the first two facts are missing. A finished issue is final in
  both, so the fourth fact is "not possible" by design and is left out of their totals. Steps are
  command-line commands; the web screens are not compared, and elapsed time is not measured.
- **History questions.** One issue goes through a claim by agent-a, a note, a release, a claim by
  agent-b, a priority change by the human and a finish. The ten questions ask who, when, on which
  branch, and what the board looked like at a moment. lific and Beads record no git branch (two
  "not possible"). With default identities they cannot tell the agents apart (three wrong).
  Quadratique answers all ten with `history query` and `issue list --asOf`.
- **Merge, deploy and checker scenarios.** A real `git merge` with no tracker command, moving
  the issue on after a deploy, and a checker's run followed by a run with one finding gone. lific
  and Beads have no status between the agent's finish and done, and no import of findings; their
  outcomes are "not possible" and "workaround", scored apart.

### What could make a row unfair

- The races count outcomes, not times, but the load of the machine that runs them changes how
  often two writes meet.
- The agent runs are one run per tracker. With three sessions, one changed decision moves a row
  by one.
- The human's question and the history questions use the tracker's command line. lific's and
  Quadratique's web UIs are not measured.
- lific's human acts through its CLI, which its log names "system". The harness accepts that as
  the human.
- lific and Beads have no roles. The `docs` issue is free for any agent in both, but `t2` blocks
  it at the start in all three.
- The MCP rows count Beads' separate `beads-mcp` package, not its recommended path.
- Token counts use tiktoken's `o200k_base`, not Claude's tokenizer. They compare the trackers;
  they are not what the model was billed.
