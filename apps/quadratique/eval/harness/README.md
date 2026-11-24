# Comparison harness: Quadratique, lific and Beads on the same scenarios

`qeval` sets up each tracker in a new copy of the `tally` repository, runs the same scripts and
the same agent sessions on all three, and writes one table with a row per measure.
[`../README.md`](../README.md) lists what the harness needs, shows how to combine runs with
`report --from`, and has a saved result and what each row measures.

From the root of the repository:

```sh
moon run quadratique-eval:test                                  # the harness's own guards
moon run quadratique-eval:scripted -- --rounds 50 --repeats 3   # no model; prints the run folder
run="<the run folder that scripted printed>"
moon run quadratique-eval:agents -- --run "$run"                # nine paid Claude Code sessions
moon run quadratique-eval:report -- --run "$run"                # results.json -> results.md
moon run quadratique-eval:check-home                            # none of the trackers' known paths exists in the real home
```

The runs live in `~/.cache/quadratique-eval/runs/` (or `$QEVAL_RUNS`), outside every repository.

## What it runs

| Tracker | Version | Setup (its quickstart) | Agents use |
|---|---|---|---|
| Quadratique | built from this source tree | `quadratique init --key TAL --name tally`, which writes the skill to `.agents/skills/quadratique/` (`SKILL.md` and `planning.md`), adds the line to `AGENTS.md` and creates the project | the `quadratique` CLI |
| lific | v2.11.1, Linux x86_64 release | `lific init --no-service --auth-mode login-free --name Human`, `lific bind TAL --create`, `lific connect --client claude-code --scope project --stdio --yes`, `lific agents-md --project TAL` | its MCP tools |
| Beads | v1.3.4, Linux amd64 release | `bd init` | the `bd` CLI |

After the setup, the maintainer commits whatever the setup left in the working tree, so that
agent worktrees see it. Beads commits by itself; for lific this commits `.mcp.json` with the
plaintext key that `lific connect` wrote.

Then the maintainer creates the eight issues of `seed-tally.sh` with their priorities, the
human's decision issue, and the four `blocks` links. The human acts through each tracker's own
command line. lific and Beads have no roles: the issue for the role `docs` stays free for any
agent in both (another issue blocks it at the start, in all three).

## Files

| File | What it does |
|---|---|
| `qeval/sandbox.py` | Downloads the pinned releases and checks their sha256; builds Quadratique into its own target folder, `target/qeval`; the run folder, the throwaway homes, the wrappers |
| `qeval/tally.py` | The `tally` repository and its eight issues |
| `qeval/adapters/{quadratique,lific,beads}.py` | One adapter per tracker; `adapters/contract.py` defines the interface each one implements, and `adapters/common.py` holds what they share |
| `qeval/scripted.py` | The measures without a model |
| `qeval/written.py` | Which files the setup wrote into the repository, and how many count |
| `qeval/agents.py` | The agent driver and what the transcripts show |
| `qeval/report.py` | The table |

## Safety

The harness keeps the trackers inside the run folder by convention and by environment, not by
a boundary the operating system enforces. What holds, and what does not:

- Every binary of the three trackers runs through a wrapper in `<run>/<tool>/bin`. The wrapper
  sets a throwaway `HOME`, the XDG folders, `DO_NOT_TRACK=1` and `QUADRATIQUE_HOME`, then runs the
  real binary. The agent sessions get the wrapper folder first on `PATH`, so the hooks and MCP
  entries that the trackers write, which call `bd` and `lific` by name, use the wrappers too.
- Every git command the harness runs in a tracker's repository has the wrappers' environment
  (`Sandbox.env()`), so a hook that a tracker installed finds the wrapper. The harness's own
  reads and checks (`rev-parse`, `diff`, the completion check's worktree) run no hook at all
  (`core.hooksPath=/dev/null`). Only the maintainer's acts (the commits after setup and
  seeding, the agents' worktrees, a merge) run the tracker's hooks, as they would for a person.
- Every process the harness starts runs in its own process group with one owner
  (`qeval/processes.py`): its group ends with it, a read from it has a hang guard, and a setup
  that fails part-way stops the tracker's server. A process that leaves its group on purpose
  escapes that owner: Quadratique's server does, and the adapter stops it with
  `quadratique server stop`.
- lific runs with `--no-service`: without it, `init` installs a systemd user unit, and the
  user's systemd manager is outside any throwaway home.
- The binaries are cached in `harness/.cache/`, which git ignores.
- `qeval check-home` is a presence check: it reports which of the trackers' known paths exist
  in the real home now (`~/.beads`, `~/.config/bd`, `~/.dolt`, `~/.config/lific`,
  `~/.local/share/lific` and any lific systemd unit), and fails when one does. It cannot tell a
  path a run wrote from one that was there before, so run it before the paid sessions too: it
  must pass then. Every run ends with that check.

**What is not closed.** The agent sessions run with the real `HOME`, because Claude Code's
sign-in is there, and with `--permission-mode bypassPermissions`: a session may run any
command as the user. It can call a tracker by its full path and skip the wrapper, read or
write any file of the user, and reach the network. The harness cannot close this. What limits
it: the wrappers come first on `PATH`, so a tracker called by name runs inside the throwaway
home; the denied tools (below) stop a session from starting other sessions or leaving its
worktree through Claude Code; the prompt names only the repository's `AGENTS.md`; and
`check-home` after each run, passing before it, would show a tracker file written into the real home. What does not
limit it: nothing stops a command that names a path outside the run folder, and `check-home`
looks only at the trackers' known paths.

## Fairness rules

They apply the same way to all three trackers.

1. **Default profile.** Each tracker runs as its quickstart sets it up, in the mode it
   recommends to agents: Quadratique and Beads through their command lines, lific through MCP.
   The other path is measured as a second row where it changes a number (the MCP tool list).
2. **Same inputs.** The same repository, issues, relations and priorities; the same prompt,
   model (`claude-opus-5-5`), effort (medium) and denied tools for every session.
3. **What the harness sets for agents.** Nothing beyond what the tracker's setup wrote. Each
   session starts in its own worktree with a clean environment. Two settings are real choices,
   and both get a second row, never the headline:
   - Beads takes the git user as every agent's identity unless `BEADS_ACTOR` is set. Its
     instructions to agents do not mention it. The headline leaves it unset.
   - lific has no claim. Two agents that set an issue `active` both succeed unless they pass
     `expected_seq`, which its instructions do not mention. The headline does not pass it.
   - For symmetry, the "one identity per agent" row gives lific a bot and a key per agent and
     Quadratique a session id per agent. Quadratique already names an agent after its worktree.
4. **Repeat what varies.** The races run several times, and the table lists the value of each
   repeat. The agent runs are few, and the table lists each run's value.
5. **"Not possible" stands apart.** A question a tracker cannot answer by design (lific and
   Beads record no git branch; their finishing call is final, so nothing waits for review) is
   scored "not possible", apart from right and wrong. A change the tracker made but does not
   show is wrong.
6. **One tokenizer.** Every token count uses tiktoken's `o200k_base`, which is not Claude's
   tokenizer. The counts compare the trackers; they are not what the model was billed. The
   billed totals of each session are kept in `results.json`, and the table shows the first
   request's billed input as a check.
7. **Count what reached the model.** Token rows come from the transcripts: what Claude Code
   loaded before the first request (`CLAUDE.md`, or `AGENTS.md` where there is no `CLAUDE.md`;
   hook output; MCP instructions and tool names, or whole definitions when they cannot be
   deferred), every read of the tracker's instruction text, every ToolSearch fetch of its tool
   definitions, and every tracker call. Each piece counts once, when it entered the context.
8. **Completed means checked.** A session completes at most one task: the session's transcript
   has a successful finishing call on the issue (the harness reads the issue ids from those
   calls), the tracker shows the issue finished when the harness checks after the session, the
   issue is a seeded task, the
   agent's branch changes a file that task is about, and `python3 -m unittest` passes on that
   branch with at least one test run and none skipped that the task was to enable. A task whose
   issue asks for tests also needs a `.py` file under `tests/` that the branch adds or modifies.
   The harness does not check which actor finished the issue. What the agent says does not
   count.
9. **Bounds, not exact counts.** Every token row of the agent runs is a lower and an upper bound
   by one rule for all three trackers: the upper bound counts every call that touches the
   tracker whole, the lower bound only what is certainly the tracker's (`qeval/agents.py`,
   `tracker_calls`). A row is won or lost only when the runs do not overlap (`qeval/report.py`).
10. **The tracker's own record decides what it did.** Double claims in the agent runs come from
    each tracker's record of takes; a record that cannot tell the sessions apart establishes
    nothing, and the row says so instead of reporting zero.
11. **One skill is one file.** In "Files written into the repository", all files inside one skill
    folder (`.agents/skills/<name>/`, `.claude/skills/<name>/` or `.codex/skills/<name>/`) count
    as one file when its `SKILL.md` is among them; any other file counts on its own, and a rename
    counts as its deleted source and its added destination. So a skill that keeps text some
    sessions need in a file of its own costs nothing more (`qeval/written.py`, `counted_files`).

## Agent sessions

Each session runs:

```sh
claude -p "Read its \`AGENTS.md\` and follow it. Do one task from start to finish, then stop." \
  --model claude-opus-5-5 --effort medium --output-format stream-json --verbose \
  --setting-sources project,local \
  --settings '{"claudeMdExcludes":["~/.claude/CLAUDE.md","~/.agents/AGENTS.md"],"autoMemoryEnabled":false}' \
  --permission-mode bypassPermissions --disable-slash-commands --no-session-persistence \
  --disallowedTools Task,Workflow,EnterWorktree,ExitWorktree,CronCreate,RemoteTrigger,ScheduleWakeup,SendMessage,PushNotification \
  --strict-mcp-config [--mcp-config <worktree>/.mcp.json]   # lific only: the file its connect wrote
```

- `--setting-sources project,local` loads the repository's `.claude/settings.json`, where Beads
  puts its SessionStart hook, and leaves out the user's settings, hooks and plugins.
- `--setting-sources` alone still loads the user's `~/.claude/CLAUDE.md`; `claudeMdExcludes`
  removes it. A probe in an empty folder showed both.
- `--strict-mcp-config` leaves out the user's MCP servers. For lific, `--mcp-config` names the
  `.mcp.json` that `lific connect` wrote into the repository, unchanged.
- The denied tools would start other sessions or leave the worktree, where the transcript
  cannot see the work.
- The environment is clean: the real `HOME` (the sign-in lives there), the wrapper folder first
  on `PATH`, no variable from the session that runs the harness.
