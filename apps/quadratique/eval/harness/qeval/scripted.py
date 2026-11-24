from __future__ import annotations

import threading
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

from qeval import sandbox, tally, tokens, written
from qeval.adapters.common import FINGERPRINT, MISSING, NOT_POSSIBLE
from qeval.adapters.contract import Adapter
from qeval.mcp import Session

AGENTS = ["agent-a", "agent-b", "agent-c"]
HUMAN_NAMES = ("maintainer", "human", "system")  # lific's CLI acts as "system", its operator


def now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")  # to the second, as all three take it


class Prepared:
    def __init__(self, adapter: Adapter, root: Path, quadratique_bin: Path, worktrees: int = 2):
        self.adapter = adapter
        self.sb = sandbox.Sandbox(root, adapter.NAME, adapter.binaries(quadratique_bin)).create()
        self.repo = root / "tally"
        try:
            self._set_up(root, worktrees)
        except BaseException:
            self.stop()
            raise

    def _set_up(self, root: Path, worktrees: int) -> None:
        env = self.sb.env()
        tally.create_repo(self.repo, env)
        self.base = tally.git(self.repo, "rev-parse", "HEAD", env=env).strip()
        self.config_before = (self.repo / ".git" / "config").read_text()
        self.adapter.setup(self.sb, self.repo)
        self.committed_by_maintainer = tally.commit_all(self.repo, env, "Set up the issue tracker")
        self.ids = self.adapter.seed(self.sb, self.repo)
        tally.commit_all(self.repo, env, "Seed the issues")  # Beads' hooks may leave an export
        self.worktrees = {}
        for agent in AGENTS[:worktrees]:
            path = root / agent
            tally.git(self.repo, "worktree", "add", "-q", "-b", agent, str(path), "main", env=env, hooks=True)
            self.worktrees[agent] = path

    def env(self, agent: str, distinct: bool) -> dict:
        return self.adapter.agent_env(self.sb, self.repo, agent, distinct)

    def stop(self) -> None:
        if self.repo.exists():
            self.adapter.stop(self.sb, self.repo)

    def __enter__(self) -> "Prepared":
        return self

    def __exit__(self, *exc) -> None:
        self.stop()


def measure_setup(p: Prepared) -> dict:
    env = p.sb.env()
    setup_head = tally.git(p.repo, "rev-list", "--max-count=1", "--grep=Seed the issues", "--invert-grep", "HEAD", env=env).strip()
    files = written.name_status(p.repo, p.base, setup_head, env)
    commits = tally.git(p.repo, "log", "--format=%an: %s", f"{p.base}..{setup_head}", env=env).strip().splitlines()
    config_after = (p.repo / ".git" / "config").read_text()
    steps = [" ".join(entry["cmd"]) for entry in p.sb.log]
    if p.committed_by_maintainer:
        steps.append("git add -A && git commit (the maintainer commits what the setup wrote)")
    return {
        "steps": steps,
        "step_count": len(steps),
        "files": files,
        "commits": commits,
        "git_config_added": [line.strip() for line in config_after.splitlines() if line not in p.config_before.splitlines()],
        "home_files": sandbox.home_files(p.sb.home),
        "tool_home_files": sandbox.home_files(p.sb.root / "qhome"),
    }


def _race(p: Prepared, rounds: int, prepare, act, judge) -> dict:
    rows = []
    for i in range(rounds):
        issue, inputs = prepare(i)
        barrier = threading.Barrier(2)
        out = {}

        def go(agent):
            barrier.wait()
            out[agent] = act(agent, issue, inputs[agent], i)

        threads = [threading.Thread(target=go, args=(a,)) for a in ("agent-a", "agent-b")]
        [t.start() for t in threads]
        [t.join() for t in threads]
        rows.append({"issue": issue, "a": out["agent-a"], "b": out["agent-b"], **judge(issue, out, i)})
    return {"rounds": rounds, "rows": rows}


def double_claims(p: Prepared, rounds: int, distinct: bool, cas: bool = False) -> dict:
    a = p.adapter
    envs = {agent: p.env(agent, distinct) for agent in ("agent-a", "agent-b")}

    def prepare(i):
        issue = a.new_issue(p.sb, p.repo, f"race {i}")
        inputs = {k: (a.prepare_cas(p.sb, p.repo, issue, v) if cas else v) for k, v in envs.items()}
        return issue, inputs

    result = _race(p, rounds, prepare,
                   lambda agent, issue, env, i: a.claim(p.sb, p.worktrees[agent], issue, env),
                   lambda issue, out, i: {"holder": a.holder(p.sb, p.repo, issue)})
    rows = result["rows"]
    result |= {"double": sum(r["a"] == "ok" and r["b"] == "ok" for r in rows),
               "one": sum((r["a"] == "ok") != (r["b"] == "ok") for r in rows),
               "none": sum(r["a"] != "ok" and r["b"] != "ok" for r in rows),
               "errors": sum((r["a"] == "error") + (r["b"] == "error") for r in rows)}
    return result


def lost_updates(p: Prepared, rounds: int, kind: str, distinct: bool = False, cas: bool = False) -> dict:
    a = p.adapter
    envs = {agent: p.env(agent, distinct) | ({"cas": "1"} if cas else {}) for agent in ("agent-a", "agent-b")}
    write, read = (a.append, a.appended) if kind == "append" else (a.rewrite, a.rewritten)

    def judge(issue, out, i):
        text = read(p.sb, p.repo, issue)
        lost = [agent for agent in out if out[agent] == "ok" and f"{kind}-from-{agent}-{i}" not in text]
        return {"lost": lost}

    result = _race(p, rounds, lambda i: (a.new_issue(p.sb, p.repo, f"{kind} {i}"), envs),
                   lambda agent, issue, env, i: write(p.sb, p.worktrees[agent], issue, env, f"{kind}-from-{agent}-{i}"),
                   judge)
    rows = result["rows"]
    result |= {"lost": sum(len(r["lost"]) for r in rows),
               "refused": sum((r["a"] == "refused") + (r["b"] == "refused") for r in rows),
               "errors": sum((r["a"] == "error") + (r["b"] == "error") for r in rows),
               "writes_ok": sum((r["a"] == "ok") + (r["b"] == "ok") for r in rows)}
    return result


def first_tokens(p: Prepared) -> dict:
    texts = p.adapter.instructions(p.sb, p.repo)
    parts = {name: tokens.count(text) for name, text in texts.items()}
    cmd, extra = p.adapter.mcp_command(p.sb, p.repo)
    session = Session(cmd, p.sb.env(extra), str(p.repo))
    try:
        tools = session.tools()
        mcp = {"tools": len(tools["tools"]), "tokens": tokens.count_tools(tools),
               "instructions_tokens": tokens.count(session.instructions)}
    finally:
        session.close()
    return {"instructions": parts, "instructions_total": sum(parts.values()), "mcp": mcp,
            "texts": texts}


def history(p: Prepared, distinct: bool) -> dict:
    """Steps are a little over a second apart, because lific and Beads keep times in seconds."""
    a, sb = p.adapter, p.sb
    ea, eb = p.env("agent-a", distinct), p.env("agent-b", distinct)
    wa, wb = p.worktrees["agent-a"], p.worktrees["agent-b"]
    x = a.new_issue(sb, p.repo, "History subject")
    pause = lambda: time.sleep(1.2)
    pause(); a.claim(sb, wa, x, ea)
    pause(); m1 = now()
    pause(); a.append(sb, wa, x, ea, "note by agent-a"); a.release(sb, wa, x, ea)
    pause(); a.claim(sb, wb, x, eb)
    pause(); before = now(); a.set_priority(sb, p.repo, x, 1); after = now()
    pause(); a.finish(sb, wb, x, eb)
    answers = a.history_answers(sb, p.repo, x, {"m1": m1})
    window = (datetime.fromisoformat(before) - timedelta(seconds=1), datetime.fromisoformat(after) + timedelta(seconds=1))

    def at_in_window(value):
        moment = datetime.fromisoformat(str(value).replace("Z", "+00:00"))
        moment = moment if moment.tzinfo else moment.replace(tzinfo=timezone.utc)
        return window[0] <= moment <= window[1]

    checks = {
        "first_claim_by": lambda v: "agent-a" in v,
        "first_claim_branch": lambda v: v == "agent-a",
        "priority_changed_by": lambda v: any(n in v.lower() for n in HUMAN_NAMES),
        "priority_changed_to": lambda v: str(v).strip('"') in ("1", "urgent"),
        "priority_changed_at": at_in_window,
        "finished_by": lambda v: "agent-b" in v,
        "finished_branch": lambda v: v == "agent-b",
        "status_at_m1": lambda v: v in ("in-progress", "in_progress", "active"),
        "holder_at_m1": lambda v: "agent-a" in v,
        "started_at_m1": lambda v: v == [x],
    }
    scored = {}
    for question, check in checks.items():
        value = answers[question]
        scored[question] = {"answer": value,
                            "score": "not possible" if value == NOT_POSSIBLE
                            else "wrong" if value == MISSING else ("right" if check(value) else "wrong")}
    counts = {s: sum(v["score"] == s for v in scored.values()) for s in ("right", "wrong", "not possible")}
    return {"questions": scored, **counts, "commands": answers["_commands"]}


def human_question(p: Prepared, distinct: bool) -> dict:
    a, sb = p.adapter, p.sb
    ids = p.ids
    a.claim(sb, p.worktrees["agent-a"], ids["t1"], p.env("agent-a", distinct))
    a.claim(sb, p.worktrees["agent-b"], ids["t7"], p.env("agent-b", distinct))
    ec = p.env("agent-c", distinct)
    a.claim(sb, p.worktrees["agent-c"], ids["t8"], ec)
    a.finish(sb, p.worktrees["agent-c"], ids["t8"], ec)
    commands, rows = a.human_view(sb, p.repo)
    find = lambda issue: [r for r in rows if r["id"] == issue]
    facts = {
        "agent-a works on t1": any("agent-a" in r["holder"] for r in find(ids["t1"])),
        "agent-b works on t7": any("agent-b" in r["holder"] for r in find(ids["t7"])),
        "t4 waits on the human (assigned)": any(r["waits_on_human"] for r in find(ids["t4"])),
        "t8 waits on the human (review)": any(r["waits_on_human"] for r in find(ids["t8"])),
    }
    by_design = {"t8 waits on the human (review)"} if not a.FINISH_WAITS_FOR_REVIEW else set()
    return {"commands": commands, "facts": facts, "rows": rows,
            "right": sum(v for k, v in facts.items() if k not in by_design),
            "not_possible": sorted(by_design), "of": len(facts) - len(by_design)}


FIXTURES = Path(__file__).resolve().parents[1] / "fixtures"
MERGE_LIMIT = 30.0


def merge_and_deploy(p: Prepared) -> dict:
    a, sb = p.adapter, p.sb
    issue, worktree, env = p.ids["t8"], p.worktrees["agent-a"], p.env("agent-a", False)
    a.claim(sb, worktree, issue, env)
    version = worktree / "tally" / "__init__.py"
    version.write_text(version.read_text() + '__version__ = "0.1.0"\n')
    git_env = sb.env(env)
    tally.git(worktree, "commit", "-q", "-am", f"Add the version\n\nRefs: {issue}", env=git_env, hooks=True)
    a.finish(sb, worktree, issue, env)
    before = a.status(sb, p.repo, issue)
    tally.git(p.repo, "merge", "-q", "--no-ff", "-m", "Merge agent-a", "agent-a", env=sb.env(), hooks=True)
    first = a.status(sb, p.repo, issue)
    reads, shown = 1, first
    deadline = time.monotonic() + MERGE_LIMIT
    while a.MERGED and shown != a.MERGED and time.monotonic() < deadline:
        time.sleep(0.5)
        reads, shown = reads + 1, a.status(sb, p.repo, issue)
    merge = {"before_merge": before, "first_read": first, "shown": shown, "reads": reads,
             "score": NOT_POSSIBLE if not a.MERGED else ("right" if shown == a.MERGED else "wrong")}
    deploy = a.after_deploy(sb, p.repo, issue) if merge["score"] == "right" or not a.MERGED else \
        {"commands": [], "outcome": "not reached: the merge was not shown"}
    return {"merge": merge, "deploy": deploy}


def findings(p: Prepared) -> dict:
    a, sb = p.adapter, p.sb
    gone, stays = "f:a420556f407006e2", "f:49f3eaf2628a1f35"

    def states() -> dict[str, bool]:
        out = {}
        for key, finished in a.finding_states(sb, p.repo).items():
            found = FINGERPRINT.search(key)
            out[found.group(0) if found else key] = finished
        return out

    first = a.import_findings(sb, p.repo, FIXTURES / "findings-1.json")
    after_first = states()
    second = a.import_findings(sb, p.repo, FIXTURES / "findings-2.json")
    after_second = states()
    right = (set(after_first) == {gone, stays} and not any(after_first.values())
             and after_second.get(gone) is True and after_second.get(stays) is False)
    return {"outcome": first["outcome"], "commands": [first["commands"], second["commands"]],
            "after_first": after_first, "after_second": after_second,
            "score": ("right" if right else "wrong") if first["outcome"] == "import" else
                     ("workaround" if right else "wrong")}
