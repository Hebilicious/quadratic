# Evaluating Quadratic

Quadratic is evaluated in two ways:

- **End-to-end runs.** Real agents use Quadratic on small test repositories, and a person (or a
  lead agent in that role) acts as the maintainer. A run shows whether the system works from the
  one line in `AGENTS.md` to merged work, and what gets in an agent's way.
- **The comparison harness.** [`harness/`](harness/README.md) runs the same scripted scenarios
  and the same agent sessions on Quadratic and on two other trackers, lific and Beads, and writes
  one table with a row per measure.

## Before you start

- The Rust toolchain, to build `quadratic` (see "Before you start" in the
  [Quadratic README](../README.md)).
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
agent session, so that the issues are created by the human. From `apps/quadratic` (the Rust
workspace, the folder with `Cargo.lock`):

```sh
cargo build --release -p quadratic --target-dir target/qeval   # not target/release: the install builds there
scratch="$(mktemp -d)"                    # a folder outside every repository
export QUADRATIC_HOME="$scratch/home"     # never your real data folder
export PATH="$PWD/target/qeval/release:$PATH"
env -u CLAUDECODE -u AI_AGENT bash eval/seed-tally.sh "$scratch/tally"
env -u CLAUDECODE -u AI_AGENT bash eval/seed-pomo.sh "$scratch/pomo"
git -C "$scratch/tally" worktree add -b agent-a "$scratch/tally-wt-a" main   # one per agent
```

Stop the server of that data folder when the run is over: `quadratic server stop`, in the same
shell.

The seed scripts create a repository and its Quadratic project as the human would:

| Script | Repository | Project | Issues |
|---|---|---|---|
| `seed-tally.sh` | `tally`, a Python word counter | `TAL` | Eight: three ready, three blocked by another issue, one decision assigned to the human, one assigned to the role `docs` |
| `seed-pomo.sh` | `pomo`, a shell work timer | `POM` | Two, both ready. A second project in the same workspace shows that a repository sees only its own work |
| `seed-review.sh` | `tally`, with Quadratic's skill and an `AGENTS.md` that asks for the maintainer's review before a merge | `TAL` | None: the run starts from a conversation |

With `seed-tally.sh` and `seed-pomo.sh`, each agent gets a worktree and this prompt, and nothing
else about Quadratic:

> Read its `AGENTS.md` and follow it. Do one task from start to finish, then stop.

The maintainer then reviews each branch, merges it, calls `quadratic close` (the issue becomes
`merged`), moves it to `done` when the checks on `main` pass, answers the issues assigned to the
human, and starts the next wave on what unblocked.

With `seed-review.sh`, the maintainer talks to two sessions and writes only what a person would
write, nothing about how to use Quadratic: a planner that writes a PRD with the maintainer and
turns it into issues, then an implementer that takes the issues one at a time and asks for review
before each merge.

## Run the comparison

From the root of the repository:

```sh
moon run quadratic-eval:test                                  # the harness's own tests
moon run quadratic-eval:scripted -- --rounds 50 --repeats 3   # no model; prints the run folder
run="<the run folder that scripted printed>"
moon run quadratic-eval:check-home                            # must pass before the paid sessions
moon run quadratic-eval:agents -- --run "$run"                # nine paid Claude Code sessions
moon run quadratic-eval:report -- --run "$run"                # results.json -> results.md
```

The run folders are in `~/.cache/quadratic-eval/runs/`, or in the folder that `QEVAL_RUNS` names.
`scripted` and `agents` write their measures to `results.json` in the run folder; they and
`report` write the table to `results.md` in it and print it. To combine runs, for
example the agent sessions of one run with the scripted measures of another, pass `--from` once
per run and `--out` for the folder of `results.md`:

```sh
moon run quadratic-eval:report -- \
  --from "agents=$HOME/.cache/quadratic-eval/runs/first" \
  --from "scripted=$HOME/.cache/quadratic-eval/runs/second:quadratic,lific" \
  --out "$HOME/.cache/quadratic-eval/runs/combined"
```

Each `--from` is `agents=`, `scripted=` or `all=`, then a run folder, then optionally `:` and the
trackers to take from it. `report` with neither `--run` nor `--from` and `--out` says what to
pass and exits with status 1. [`harness/README.md`](harness/README.md) describes the setup of
each tracker, the fairness rules and the safety limits.

### How a row is called won, tied or lost

One rule applies to every row: **no overlap between the runs.** A cell holds one value per run of
that tracker: per session for the agent rows that are per session, per repeat for the races, per
run folder otherwise, and a range (lower, upper) where the measure has bounds. Quadratic **won** a
row when every Quadratic value is better than every value of each other tracker. It **lost** to a
tracker when every value of that tracker is better than every Quadratic value. Otherwise the row
is **tied**, whatever the medians say. A tracker with no value in a row ("not possible", "not
established") is left out of the comparison, and the label says so.

`qeval report` applies the rule (`harness/qeval/report.py`).

## Saved results

The table below is a saved result: its run folders are not part of this repository, so you
cannot produce it again from here. Run the comparison to produce your own table.

It was measured under these conditions:

- lific v2.11.1 and Beads v1.3.4, the releases that the harness pins.
- Quadratic built from an earlier version of this source. Its skill has changed since, so the
  token rows of a new run differ.
- Every session ran Claude Opus 5.5 at medium effort, with the prompt above.
- lific's and Beads' agent rows come from one run of three sessions each, and Quadratic's from two
  runs of three sessions. The scripted rows come from one run of 50 rounds × 3 repeats.
- The table was combined from those runs with `report --from`. Each cell lists each run's value.

| Row | Quadratic | lific | Beads | Quadratic's result |
|---|---|---|---|---|
| Tasks completed (one value per run) | 3; 3 | 2 | 1 | won |
| Double claims, default identities (per repeat of 50) | 0; 0; 0 | 50; 50; 50 | 50; 50; 50 | won |
| Double claims, one identity per agent (second row) | 0; 0; 0 | 50; 50; 50 | 0; 0; 0 | tied with Beads |
| Double claims, lific passing `expected_seq` (second row) | – | 0; 0; 0 | – | – |
| Lost updates, two appends (per repeat of 50 writes) | 0; 0; 0 (25 refused each) | 0; 0; 0 | 0; 0; 0 | tied |
| Lost updates, two read-modify-writes | 0; 0; 0 (25 refused each) | 25; 25; 25 | 4; 6; 5 | won |
| Lost updates, read-modify-write, lific passing `expected_seq` (second row) | – | 0; 0; 0 (25 refused each) | – | – |
| Double claims in the agent runs | 0; 0 (record) | not established (transcripts: 1) | not established (transcripts: 2) | not compared |
| Lost updates in the agent runs | 0 of 3; 0 of 3 checked | 0 of 3 | 2 of 3 | tied with lific |
| Tracker instructions that reached each session | 875; 875; 875; 875; 1,510–1,640; 914 | 1,832 × 3 | 3,109 × 3 | won |
| Tracker text at the first request (second row) | 39 × 6 | 698 × 3 | 2,159 × 3 | won |
| First request's billed input (second row) | 8,131 × 3; 8,119 × 3 | 9,046 × 3 | 11,257 × 3 | won |
| Whole MCP tool list (second row) | 6,431 | 5,783 | 13,522 | lost to lific |
| Tracker tokens per distinct task (one range per run) | 2,030–2,376; 2,122–2,485 | 3,652 | 9,948–13,695 | won |
| Tracker tokens per session (second row) | 1,780–2,098; 2,185–2,593; 2,125–2,437; 2,222–2,527; 2,303–2,769; 1,840–2,159 | 2,266; 2,713; 2,325 | 3,545–4,161; 3,168–4,735; 3,235–4,799 | tied with lific |
| Setup steps | 2 | 5 | 1 | lost to Beads |
| Files written into the repository | 2 | 2 | 19 | tied with lific |
| Lines added to `.git/config` | 0 | 0 | 3 | tied with lific |
| Files written outside the repository (second row) | 6 | 7 | 1 | – |
| The human's question on the command line: facts and steps (a proxy for the human's time) | 4 of 4 facts, 2 commands | 1 of 3, 3 commands | 1 of 3, 2 commands | won |
| History questions, default identities | 10 right | 5 right, 3 wrong, 2 not possible | 5 right, 3 wrong, 2 not possible | won |
| History questions, one identity per agent (second row) | 10 right | 8 right, 2 not possible | 8 right, 2 not possible | won |
| A real merge, shown with no tracker command | right: `merged` on the first read | not possible (`done` since the agent's finish) | not possible (`closed`) | not compared |
| Moving the issue on after a deploy | 2 commands (`show`, `issue update --status done`) | not possible | not possible | not compared |
| A checker's run, then one finding gone | right: 1 import command per run | workaround: a script, 3 and 2 commands | workaround: a script, 3 and 2 commands | not compared |

One Quadratic session ran `quadratic instructions | grep -A3 review` to find how to ask for a
review. The answer is one line of JSON, so `grep` returned the whole reference: that is the
session at 1,510–1,640 tokens of instructions.

### Where Quadratic loses or ties

| Row | Why | What would move it |
|---|---|---|
| Setup steps (2 against Beads' 1) | `quadratic init` makes no commit by design: it prints the `git commit` for the person to run, and the harness counts that commit. `bd init` commits by itself | Let `init` commit its files |
| Whole MCP tool list (6,431 against 5,783) | Each tool bundles the types it uses: `issue.list`, `issue.create` and `issue.update` are 3,629 of the 6,431, and `history.query` 650. The `Date` and `Time` types, whose patterns check real dates, are 145 and 192 tokens with their descriptions, and repeat in four tools | A shorter `Date` or `Time` pattern changes what the server accepts. The definitions a session fetches for the usual path (`ready`, `claim`, `close`, `update`, `get`, `list`) are 3,967 tokens (4,617 with `history.query`), against 1,081 for lific's five |
| Tracker tokens per session (tied with lific) | The skill is the largest piece, then `claim` and `close`, which answer the whole issue. A refused claim answers the holder's claim with its worktree path; sessions that tried a held issue first paid that once or twice | A shorter skill; or answers that leave out empty arrays and the claim's worktree for the agent's route, which changes catalog types that the web app reads. More sessions per tracker would show whether the overlap is real |

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
  afterwards. Quadratic loses nothing because every update names the revision it read: the
  second of two writes at the same instant gets `Stale`, for appends too, where lific and Beads
  accept both. In the agent runs, a write counts as present only when its text is found, as whole
  words, in one written field of its issue.
- **Tracker instructions that reached each session.** Read from each transcript: the tracker
  text in context at the first request, every read of the tracker's instruction text, every
  ToolSearch fetch of its MCP tool definitions, and every run of its instructions command.
- **Tracker text at the first request.** Claude Code loads `CLAUDE.md`, and loads `AGENTS.md`
  only where the project has no `CLAUDE.md`. So Quadratic and lific sessions start with
  `AGENTS.md`, and Beads sessions start with the `CLAUDE.md` that `bd init` writes.
- **First request's billed input.** Claude's own count from the first request's usage (input,
  cache creation and cache read). It checks the row above without the harness's classification.
- **Tracker tokens.** Every piece of the instructions row, plus every tracker call's input and
  output, each counted once, when it entered the context. Every token row of the agent runs is a
  lower and an upper bound: the upper bound counts every call that touches the tracker whole, the
  lower bound only what is certainly the tracker's. No number is exact.
- **Setup steps and files.** The quickstart's commands, plus the maintainer's commit when the
  setup does not commit by itself. The files of one skill folder count as one file when its
  `SKILL.md` is among them. Beads' one step writes 19 files, three lines of `.git/config`
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
  Quadratic answers all ten with `history query` and `issue list --asOf`.
- **Merge, deploy and checker scenarios.** A real `git merge` with no tracker command, moving
  the issue on after a deploy, and a checker's run followed by a run with one finding gone. lific
  and Beads have no status between the agent's finish and done, and no import of findings; their
  outcomes are "not possible" and "workaround", scored apart.

### What could make a row unfair

- The races count outcomes, not times, but the load of the machine that runs them changes how
  often two writes meet.
- The agent runs are one or two runs per tracker. With three sessions, one changed decision moves
  a row by one.
- The human's question and the history questions use the tracker's command line. lific's and
  Quadratic's web UIs are not measured.
- lific's human acts through its CLI, which its log names "system". The harness accepts that as
  the human.
- lific and Beads have no roles. The `docs` issue is free for any agent in both, but `t2` blocks
  it at the start in all three.
- The MCP rows count Beads' separate `beads-mcp` package, not its recommended path.
- Token counts use tiktoken's `o200k_base`, not Claude's tokenizer. They compare the trackers;
  they are not what the model was billed.
