from __future__ import annotations

import json
from pathlib import Path
from typing import Literal, TypedDict

from qeval.written import counted_files

TOOLS = ["quadratic", "lific", "beads"]
SCRIPTED_KEYS = ("setup", "tokens", "human", "history_default", "history_distinct", "races", "merge", "findings")


class Sources(TypedDict):
    agents: dict[str, list[dict]]
    scripted: dict[str, dict[str, list]]


class Row(TypedDict):
    measure: str
    better: Literal["lower", "higher"] | None
    second: bool
    values: dict[str, list]
    texts: dict[str, str]
    how: str


def _range(value) -> tuple[float, float]:
    return tuple(value) if isinstance(value, (tuple, list)) else (value, value)


def _number(value) -> str:
    lo, hi = _range(value)
    return f"{lo:,.0f}" if lo == hi else f"{lo:,.0f}–{hi:,.0f}"


class Unmeasured:
    def __repr__(self) -> str:
        return "UNMEASURED"


UNMEASURED = Unmeasured()


def verdict(row: Row) -> str:
    if not row["better"]:
        return ""
    measured = {t: [v for v in row["values"][t] if v is not UNMEASURED] for t in TOOLS}
    values = {t: [v for v in measured[t] if v is not None] for t in TOOLS}
    unmeasured = [t for t in TOOLS if len(measured[t]) < len(row["values"][t])
                  or (values[t] and None in measured[t])]
    if not values["quadratic"]:
        return "quadratic not measured" if "quadratic" in unmeasured else ""
    lower = row["better"] == "lower"

    def beats(ours, theirs) -> bool:
        if lower:
            return max(_range(v)[1] for v in ours) < min(_range(v)[0] for v in theirs)
        return min(_range(v)[0] for v in ours) > max(_range(v)[1] for v in theirs)

    lost, tied, apart = [], [], []
    for tool in TOOLS[1:]:
        if not values[tool]:
            if measured[tool] and tool not in unmeasured:
                apart.append(tool)
            continue
        if beats(values[tool], values["quadratic"]):
            lost.append(tool)
        elif not beats(values["quadratic"], values[tool]):
            tied.append(tool)
    compared = any(values[t] or t in unmeasured for t in TOOLS[1:])
    won = "won" if not unmeasured else f"not won: {' and '.join(unmeasured)} not measured"
    label = (f"lost to {' and '.join(lost)}" if lost else f"tied with {' and '.join(tied)}" if tied
             else won if compared else "")
    if apart:
        label += ("; " if label else "") + f"{' and '.join(apart)} not compared (no value)"
    return label


def rows(sources: Sources) -> list[Row]:
    agents_runs = sources["agents"]
    scripted = sources["scripted"]
    out: list[Row] = []

    def add(measure, better, how, cell, second=False, group="scripted", key=None):
        values, texts = {}, {}
        for tool in TOOLS:
            runs = agents_runs.get(tool, []) if group == "agents" else scripted.get(key, {}).get(tool, [])
            pairs = []
            for run in runs:
                try:
                    pairs += cell(run, tool)
                except (KeyError, TypeError, IndexError):
                    pairs.append((UNMEASURED, "not measured"))
            values[tool] = [v for v, _ in pairs]
            texts[tool] = "; ".join(t for _, t in pairs) or "–"
        out.append({"measure": measure, "better": better, "second": second, "values": values, "texts": texts, "how": how})

    def sessions(run):
        return list(run["sessions"].values())

    def tracker(run):
        return [s["tracker"] for s in sessions(run)]

    def done(run) -> int:
        return len({s["completed"]["task"] for s in sessions(run) if s["completed"]["completed"]})

    def known(t):
        return t["start_total"] + t["instruction_reads"] + t["schema_fetches"]

    def instructions_reached(t) -> tuple[int, int]:
        return tuple(known(t) + t["instructions_command"][b] for b in ("lower", "upper"))

    def spent_by(t) -> tuple[int, int]:
        return tuple(known(t) + t["call_input"][b] + t["call_output"][b] for b in ("lower", "upper"))

    def each(values):
        return [(v, _number(v)) for v in values]

    if agents_runs:
        add("Tasks completed without human help", "higher",
            "Three parallel sessions per run, one value per run: the distinct tasks completed. A session completes at most one task: the tracker shows the issue finished after the session's own finishing call, the issue is a seeded task, the branch changes a file that task is about and, for a task whose acceptance asks for tests, a test file, and `python3 -m unittest` passes there with at least one test run and each test the task was to enable run and passed.",
            lambda run, t: [(done(run), f"{done(run)} ({sum(s['completed']['completed'] for s in sessions(run))} of {len(sessions(run))} sessions)")],
            group="agents")

    def claims(runs, t):
        return [(r["double"], f"{r['double']}" + (f" ({r['errors']} errors)" if r["errors"] else "")) for r in runs]

    def lost(runs, t):
        return [(r["lost"], f"{r['lost']}" + (f" ({r['refused']} refused)" if r["refused"] else "")) for r in runs]

    def races(name, getter, only=None):
        return lambda run, t: getter(run[name], t) if only is None or t in only else []

    add("Double claims, default identities", "lower",
        "Scripted. Two processes in two worktrees take the same new issue, released by one barrier; one value per repeat of 50 rounds.",
        races("claims_default", claims), key="races")
    add("Double claims, one identity per agent", "lower",
        "Same, with BEADS_ACTOR per agent for Beads, a bot and key per agent for lific, a session id per agent for Quadratic.",
        races("claims_distinct", claims), second=True, key="races")
    add("Double claims, lific with expected_seq", None,
        "Same as the default row, each agent passing the seq it read; only lific has this option.",
        races("claims_expected_seq", claims, only={"lific"}), second=True, key="races")
    add("Lost updates, two appends", "lower",
        "Scripted. Two agents append a note (Quadratic, Beads) or a comment (lific) to one issue at once; lost: reported done, text absent. One value per repeat of 50 writes.",
        races("append", lost), key="races")
    add("Lost updates, two read-modify-writes", "lower",
        "Scripted. Two agents read the description, add a line, and write it back at once. One value per repeat of 50 writes.",
        races("rewrite", lost), key="races")
    add("Lost updates, read-modify-write with expected_seq", None,
        "As above, lific passing the seq it read.", races("rewrite_expected_seq", lost, only={"lific"}), second=True, key="races")

    if agents_runs:
        def claimed_twice(run, t):
            record, check = run["double_claims"], len(run["double_claims_in_transcripts"])
            if not record["established"]:
                return [(None, f"not established ({record['why']}; the transcripts show {check})")]
            return [(len(record["doubles"]), f"{len(record['doubles'])} (transcripts: {check})")]
        add("Double claims in the agent runs", "lower",
            "One value per run, from each tracker's own record of who took what and when (Quadratic's history, Beads' events, lific's activity), kept in the run folder as take-record.json: a take while another session held the issue. The record decides only when it names each session apart; the count from the transcripts (each take's own answer) is shown as a check.",
            claimed_twice, group="agents")

        def lost_in_agents(run, t):
            lu = run["lost_updates"]
            return [(len(lu["lost"]), f"{len(lu['lost'])} of {lu['checked']} checked, {lu['made']} made")]
        add("Lost updates in the agent runs", "lower",
            "One value per run: texts a session wrote with success (notes, evidence, close reasons, comments), each compared whole with one text field the tracker stores now, as whole words; a text the shell built is compared by its literal parts, and one whose literal parts are under 20 characters is made but not checked. A write counts as checked only when its own result confirms it: an MCP result, a shell line whose exit code a failed tracker command would reach, or Quadratic's answer. The value covers only the checked writes.",
            lost_in_agents, group="agents")
        add("Tracker instructions that reached each session", "lower",
            "From the transcripts, o200k_base, one range per session: the tracker text in context at the first request, every read of the tracker's instruction text (AGENTS.md, CLAUDE.md, skill), every ToolSearch fetch of its MCP tool definitions (input and output), and every run of its instructions command (`quadratic instructions`, `bd prime`), whose output is a lower and an upper bound as for the calls.",
            lambda run, t: each([instructions_reached(x) for x in tracker(run)]), group="agents")
        add("Tracker text in context at the first request", "lower",
            "From the transcripts, o200k_base, one value per session: what Claude Code loaded before the first request (CLAUDE.md, or AGENTS.md where there is no CLAUDE.md; SessionStart hook output; MCP instructions and tool names, definitions deferred).",
            lambda run, t: each([x["start_total"] for x in tracker(run)]), second=True, group="agents")
        add("First request's input tokens, as billed", "lower",
            "Claude's own count from the first request's usage (input + cache creation + cache read), one value per session: a check that does not depend on the harness's classification.",
            lambda run, t: each([x["first_request_input"] for x in tracker(run)]), second=True, group="agents")

    add("Tokens of the MCP tool list", "lower",
        "Scripted, o200k_base, tools/list as compact JSON, all tools. Beads: the separate beads-mcp package; its recommended path is the CLI. Quadratic's recommended path is the CLI too.",
        lambda run, t: [(run["mcp"]["tokens"], f"{run['mcp']['tokens']:,} ({run['mcp']['tools']} tools)")],
        second=True, key="tokens")

    if agents_runs:
        def spent(run, t):
            n = done(run)
            if not n:
                return [(None, "no task completed")]
            per = [spent_by(x) for x in tracker(run)]
            value = (sum(lo for lo, _ in per) / n, sum(hi for _, hi in per) / n)
            return [(value, _number(value))]
        add("Tokens spent on the tracker per distinct task", "lower",
            "One range per run: all sessions' tracker tokens divided by the distinct tasks completed. Per session, o200k_base, each piece once when it entered the context: the tracker text at the first request, every read of its instruction text, every fetch of its MCP tool definitions (input and output), and every tracker call. One rule for the three trackers: the upper bound counts every call that touches the tracker whole; the lower bound only what is certainly the tracker's (an MCP call whole; of a shell line, its tracker commands, and its output when every command is the tracker's, a command that prints nothing, or a filter it pipes into, else only answers that can be told apart, which only Quadratic's JSON can). Nothing here is exact.",
            spent, group="agents")
        add("Tokens spent on the tracker per session", "lower",
            "The same pieces, one range per session, without dividing by tasks.",
            lambda run, t: each([spent_by(x) for x in tracker(run)]), second=True, group="agents")

    add("Setup steps", "lower", "Scripted. Commands of the quickstart, plus the maintainer's commit when the setup does not commit.",
        lambda run, t: [(run["step_count"], str(run["step_count"]))], key="setup")
    add("Files written into the repository", "lower", "Scripted. git diff --name-status --no-renames from the start commit to the end of setup; the files of one skill folder (.agents/, .claude/ or .codex/skills/<name>/) that include its SKILL.md count as one.",
        lambda run, t: [(n := counted_files(run["files"]), str(n))], key="setup")
    add("Lines added to .git/config", "lower", "Scripted. The repository's .git/config before and after setup.",
        lambda run, t: [(len(run["git_config_added"]), str(len(run["git_config_added"])))], key="setup")
    add("Files written outside the repository", None,
        "Scripted. Files in the throwaway HOME and in QUADRATIC_HOME after setup and seeding: each tracker's store and config.",
        lambda run, t: [(None, str(len(run["home_files"]) + len(run["tool_home_files"])))], second=True, key="setup")

    def answer(run, t):
        h = run["default"]
        text = f"{h['right']} of {h['of']} facts, {h['commands']} commands"
        if h["not_possible"]:
            text += f" (not possible: {', '.join(h['not_possible'])})"
        return [((h["of"] - h["right"]) * 100 + h["commands"], text)]
    add("\"What is every agent doing, and what waits on me?\" on the command line: facts and steps (a proxy for the human's time)", "lower",
        "Scripted, command line, default identities. Four facts: two agents' issues, the issue assigned to the human, the finished issue that waits for review; and the number of commands, a count of steps that stands in for the human's time. Elapsed human time was not measured, nor the web UIs. Ordered by facts missed, then commands.",
        answer, key="human")

    for key, label, second in (("history_default", "History questions, default identities", False),
                               ("history_distinct", "History questions, one identity per agent", True)):
        add(label, "higher", "Scripted. Ten questions on a scripted sequence (who, when, which branch, the board at a moment), answered from the tracker's commands.",
            lambda run, t: [(run["right"], f"{run['right']} right, {run['wrong']} wrong, {run['not possible']} not possible ({run['commands']} commands)")],
            second=second, key=key)

    def merged(run, t):
        m = run["merge"]
        text = f"{m['score']}: `{m['shown']}` ({m['reads']} read{'s' if m['reads'] > 1 else ''}; `{m['before_merge']}` before the merge)"
        return [({"right": 1, "wrong": 0}.get(m["score"]), text)]
    add("A real merge, shown without anyone telling the tracker", "higher",
        "Scripted. agent-a finishes t8 with a commit that ends `Refs: <issue>`; the maintainer runs `git merge --no-ff` into main and calls no tracker command; then the tracker's own status read, polled up to 30 s (a hang guard). Right (1): a status that says merged and not done. \"not possible\": the tracker has no such status (its finishing call is final), scored apart.",
        merged, key="merge")

    def deployed(run, t):
        d = run["deploy"]
        if d["outcome"] == "not possible":
            return [(None, f"not possible: shows `{d['shows']}` since the agent's finish")]
        if d["outcome"] != "done":
            return [(UNMEASURED, f"not scored: {d['outcome']}")]
        return [(len(d["commands"]), f"{len(d['commands'])} commands to `{d['outcome']}`")]
    add("Moving the issue on after a deploy (\"merged but not done\")", "lower",
        "Scripted, after the merge row: the commands that move the merged issue to done once a deploy passed, or \"not possible\" when no status lies between the agent's finish and done.",
        deployed, key="merge")

    def findings_row(run, t):
        runs = " and ".join(str(len(c)) for c in run["commands"])
        how = "its import" if run["outcome"] == "import" else "a script around its own commands"
        return [({"right": 1}.get(run["score"]), f"{run['score']}: {runs} commands, through {how}")]
    add("Findings from a checker: a run, then a run with one finding gone", "higher",
        "Scripted. A fixed run of a linter with two findings (`harness/fixtures/`), then the same run without one. Right (1): two open issues after the first run; after the second, the gone finding's issue finished and the other open. A tracker with no import of findings goes through a script that lists, creates and closes: \"workaround\", scored apart.",
        findings_row, key="findings")
    return out


def load(specs: list[tuple[str, Path, list[str]]]) -> Sources:
    sources: Sources = {"agents": {}, "scripted": {}}
    for kind, run, tools in specs:
        results = run / "results.json"
        try:
            text = results.read_text()
        except FileNotFoundError:
            raise SystemExit(f"qeval report: {results} does not exist. Pass a run folder that qeval scripted or "
                             "qeval agents wrote.") from None
        except OSError as error:
            raise SystemExit(f"qeval report: cannot read {results}: {error.strerror or error}") from None
        data = json.loads(text)
        for tool in tools:
            if kind in ("agents", "all") and tool in data.get("agents", {}):
                sources["agents"].setdefault(tool, []).append(data["agents"][tool])
            if kind in ("scripted", "all"):
                for key in SCRIPTED_KEYS:
                    if tool in data.get(key, {}):
                        sources["scripted"].setdefault(key, {}).setdefault(tool, []).append(data[key][tool])
    return sources


def table(sources: Sources) -> str:
    lines = ["| Measure | Quadratic | lific | Beads | Quadratic | How it was measured |", "|---|---|---|---|---|---|"]
    for row in rows(sources):
        if all(text == "–" for text in row["texts"].values()):
            continue
        label = row["measure"] + (" (second row)" if row["second"] else "")
        cells = [row["texts"][t] for t in TOOLS]
        lines.append(f"| {label} | {' | '.join(cells)} | {verdict(row)} | {row['how']} |")
    return "\n".join(lines) + "\n"


def parse_from(spec: str) -> tuple[str, Path, list[str]]:
    kind, _, rest = spec.partition("=")
    path, _, tools = rest.partition(":")
    if kind not in ("agents", "scripted", "all") or not path:
        raise SystemExit(f"--from wants agents=, scripted= or all=<run folder>[:tool,...], not {spec!r}")
    return kind, Path(path).expanduser(), tools.split(",") if tools else TOOLS


def write(run: Path | None = None, specs: list[tuple[str, Path, list[str]]] | None = None,
          out: Path | None = None) -> str:
    text = table(load(specs or [("all", run, TOOLS)]))
    target = (out or run) / "results.md"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text)
    return text
