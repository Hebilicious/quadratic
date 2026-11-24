from __future__ import annotations

import json
import re
from datetime import datetime, timezone
from pathlib import Path

from qeval import tally
from qeval.adapters import common
from qeval.adapters.common import MISSING, NOT_POSSIBLE, Outcome
from qeval.mcp import Session
from qeval.sandbox import Sandbox, fetch

NAME = "lific"
FINISH_WAITS_FOR_REVIEW = False
FINISHED = {"done"}
PRIORITY = {1: "urgent", 2: "high", 3: "medium", 4: "low"}
# "- [2030-01-15 16:39:11] Claude Code (agent) via mcp — TAL-1 status: todo → active", or
# "- [2030-01-15 16:37:36] system via cli — created issue TAL-1: x" (the CLI's operator has no kind)
ACTIVITY = re.compile(r"^- \[(?P<at>[^\]]+)\] (?P<who>.+?)(?: \((?P<kind>\w+)\))? via (?P<via>\w+) — (?P<what>.*)$")

_sessions: dict[tuple[str, str], Session] = {}
_keys: dict[tuple[str, str], str] = {}


def binaries(quadratique: Path) -> dict[str, Path]:
    return {"lific": fetch("lific")}


def lific(sb: Sandbox, cwd: Path, *args: str, check: bool = True, record: bool = False):
    return sb.run(["lific", *args], cwd, check=check, record=record)


def lj(sb: Sandbox, cwd: Path, *args: str):
    return json.loads(lific(sb, cwd, *args, "--json").stdout)


def setup(sb: Sandbox, repo: Path) -> None:
    lific(sb, repo, "init", "--no-service", "--auth-mode", "login-free", "--name", "Human", record=True)
    lific(sb, repo, "bind", "TAL", "--create", record=True)
    lific(sb, repo, "connect", "--client", "claude-code", "--scope", "project", "--stdio", "--yes", record=True)
    lific(sb, repo, "agents-md", "--project", "TAL", record=True)


def seed(sb: Sandbox, repo: Path) -> dict[str, str]:
    ids = {}
    for spec in tally.ISSUES:
        args = ["issue", "create", "--project", "TAL", "--title", spec.title, "--status", "todo",
                "--priority", PRIORITY[spec.priority],
                "--description", spec.description + (f"\n\nAcceptance: {spec.acceptance}" if spec.acceptance else "")]
        if isinstance(spec.assignee, tally.Human):
            args += ["--assign", "human"]
        # lific has no roles: the docs issue stays free for any agent.
        ids[spec.key] = lj(sb, repo, *args)["identifier"]
    for spec in tally.ISSUES:
        for target in spec.blocks:
            lific(sb, repo, "issue", "link", "--type", "blocks", ids[spec.key], ids[target])
    return ids


def _mcp_entry(repo: Path) -> dict:
    return json.loads((repo / ".mcp.json").read_text())["mcpServers"]["lific"]


def _default_token(repo: Path) -> str:
    return _mcp_entry(repo)["env"]["LIFIC_TOKEN"]


def _session(sb: Sandbox, cwd: Path, token: str) -> Session:
    key = (str(cwd), token)
    if key not in _sessions:
        entry = _mcp_entry(cwd if (cwd / ".mcp.json").exists() else _main(sb, cwd))
        _sessions[key] = Session([entry["command"], *entry["args"]], sb.env({"LIFIC_TOKEN": token}), str(cwd))
    return _sessions[key]


def _call(sb: Sandbox, cwd: Path, token: str, tool: str, args: dict) -> dict:
    """lific answers a refusal as ordinary text that starts with "Error:", not as an MCP error,
    so that text counts as a failure too."""
    answer = _session(sb, cwd, token).call(tool, args)
    return {"ok": answer["ok"] and not answer["text"].startswith("Error"), "text": answer["text"]}


def _main(sb: Sandbox, cwd: Path) -> Path:
    common = Path(tally.git(cwd, "rev-parse", "--path-format=absolute", "--git-common-dir", env=sb.env()).strip())
    return common.parent


def agent_env(sb: Sandbox, repo: Path, agent: str, distinct: bool) -> dict:
    """By default every Claude Code session uses the one key `lific connect` minted."""
    if not distinct:
        return {"LIFIC_TOKEN": _default_token(repo)}
    if (str(repo), agent) in _keys:
        return {"LIFIC_TOKEN": _keys[(str(repo), agent)]}
    users = lj(sb, repo, "user", "list")
    if not any(u["username"] == agent for u in users):
        lific(sb, repo, "user", "create", "--username", agent, "--email", f"{agent}@example.invalid", "--bot",
                  "--password", "bot-without-login")
        lific(sb, repo, "member", "add", "--user", agent, "--project", "TAL", "--role", "maintainer")
    key = lj(sb, repo, "key", "create", "--name", f"{agent}-{len(_keys)}", "--user", agent)["key"]
    _keys[(str(repo), agent)] = key
    return {"LIFIC_TOKEN": key}


def new_issue(sb: Sandbox, repo: Path, title: str, description: str = "base") -> str:
    return lj(sb, repo, "issue", "create", "--project", "TAL", "--title", title, "--status", "todo",
              "--description", description)["identifier"]


def _seq(sb, cwd, issue) -> int:
    return lj(sb, cwd, "issue", "get", issue)["seq"]


def claim(sb: Sandbox, cwd: Path, issue: str, env: dict) -> Outcome:
    """lific has no claim: an agent takes an issue by setting it active."""
    args = {"identifier": issue, "status": "active"}
    if env.get("cas"):
        args["expected_seq"] = env["cas_seq"]
    answer = _call(sb, cwd, env["LIFIC_TOKEN"], "update_issue", args)
    return "ok" if answer["ok"] else ("refused" if "has changed since seq" in answer["text"] else "error")


def prepare_cas(sb, repo, issue, env) -> dict:
    return {**env, "cas": "1", "cas_seq": _seq(sb, repo, issue)}


def activity(sb: Sandbox, repo: Path, issue: str) -> list[dict]:
    text = _session(sb, repo, _default_token(repo)).call("get_activity", {"identifier": issue, "limit": 500})["text"]
    rows = [m.groupdict() for m in map(ACTIVITY.match, text.splitlines()) if m]
    for row in rows:
        row["at"] = datetime.fromisoformat(row["at"]).replace(tzinfo=timezone.utc)
    return list(reversed(rows))


def holder(sb: Sandbox, repo: Path, issue: str) -> str:
    movers = [r["who"] for r in activity(sb, repo, issue) if "status:" in r["what"] and r["what"].endswith("→ active")]
    return ",".join(movers)


def append(sb: Sandbox, cwd: Path, issue: str, env: dict, text: str) -> Outcome:
    answer = _call(sb, cwd, env["LIFIC_TOKEN"], "add_comment", {"identifier": issue, "content": text})
    return "ok" if answer["ok"] else "error"


def appended(sb: Sandbox, repo: Path, issue: str) -> str:
    return _session(sb, repo, _default_token(repo)).call("get_issue", {"identifier": issue, "include_comments": "all"})["text"]


def rewrite(sb: Sandbox, cwd: Path, issue: str, env: dict, line: str) -> Outcome:
    current = lj(sb, cwd, "issue", "get", issue)
    args = {"identifier": issue, "description": (current.get("description") or "") + "\n" + line}
    if env.get("cas"):
        args["expected_seq"] = current["seq"]
    answer = _call(sb, cwd, env["LIFIC_TOKEN"], "update_issue", args)
    return "ok" if answer["ok"] else ("refused" if "has changed since seq" in answer["text"] else "error")


def rewritten(sb: Sandbox, repo: Path, issue: str) -> str:
    return lj(sb, repo, "issue", "get", issue).get("description") or ""


def release(sb, cwd, issue, env):
    _call(sb, cwd, env["LIFIC_TOKEN"], "update_issue", {"identifier": issue, "status": "todo"})


def set_priority(sb, repo, issue, priority: int):
    lific(sb, repo, "issue", "update", issue, "--priority", PRIORITY[priority])


def finish(sb, cwd, issue, env):
    _call(sb, cwd, env["LIFIC_TOKEN"], "update_issue", {"identifier": issue, "status": "done", "evidence": "tests pass"})


def _status_at(rows: list[dict], moment: datetime, initial: str = "todo") -> tuple[str, str]:
    status, who = initial, ""
    for row in rows:
        found = re.search(r"status: (\w+) → (\w+)$", row["what"])
        if found and row["at"] <= moment:
            status, who = found.group(2), row["who"]
    return status, who if status == "active" else ""


def history_answers(sb: Sandbox, repo: Path, x: str, marks: dict) -> dict:
    rows = activity(sb, repo, x)
    takes = [r for r in rows if re.search(r"status: \w+ → active$", r["what"])]
    priority = [r for r in rows if " priority: " in r["what"]]
    done = [r for r in rows if re.search(r"status: \w+ → done$", r["what"])]
    m1 = datetime.fromisoformat(marks["m1"])
    status, who = _status_at(rows, m1)
    issues = lj(sb, repo, "issue", "list", "--project", "TAL")
    started = sorted(i["identifier"] for i in issues if _status_at(activity(sb, repo, i["identifier"]), m1)[0] == "active")
    return {
        "first_claim_by": takes[0]["who"] if takes else MISSING,
        "first_claim_branch": NOT_POSSIBLE,  # lific records no git branch
        "priority_changed_by": priority[-1]["who"] if priority else MISSING,
        "priority_changed_to": priority[-1]["what"].rsplit("→ ", 1)[1] if priority else MISSING,
        "priority_changed_at": priority[-1]["at"].isoformat() if priority else MISSING,
        "finished_by": done[-1]["who"] if done else MISSING,
        "finished_branch": NOT_POSSIBLE,
        "status_at_m1": status,
        "holder_at_m1": who,
        "started_at_m1": started,
        "_commands": 2 + len(issues),
    }


def human_view(sb: Sandbox, repo: Path) -> tuple[int, list[dict]]:
    """`lific issue list` shows each issue's status and who it is for, but not who works on it:
    that is in each active issue's activity log, one more command per active issue."""
    issues = lj(sb, repo, "issue", "list", "--project", "TAL")
    rows, commands = [], 1
    for item in issues:
        if item["status"] == "active":
            commands += 1
            rows.append({"id": item["identifier"], "status": "active", "holder": holder(sb, repo, item["identifier"]),
                         "branch": "", "waits_on_human": False})
        elif item.get("needs_human") and item["status"] not in FINISHED:
            rows.append({"id": item["identifier"], "status": item["status"], "holder": "", "branch": "",
                         "waits_on_human": True})
    return commands, rows


def instructions(sb: Sandbox, repo: Path) -> dict[str, str]:
    agents = (repo / "AGENTS.md").read_text().replace(tally.FILES["AGENTS.md"], "", 1)
    return {"AGENTS.md block": agents, "MCP server instructions": _session(sb, repo, _default_token(repo)).instructions}


def mcp_command(sb: Sandbox, repo: Path) -> tuple[list[str], dict]:
    entry = _mcp_entry(repo)
    return [entry["command"], *entry["args"]], {"LIFIC_TOKEN": entry["env"]["LIFIC_TOKEN"]}


def finished(sb: Sandbox, repo: Path, issue: str) -> bool:
    return lj(sb, repo, "issue", "get", issue)["status"] in FINISHED


def board(sb: Sandbox, repo: Path) -> list[dict]:
    return [{"id": i["identifier"], "title": i["title"], "status": i["status"], "finished": i["status"] in FINISHED,
             "holder": holder(sb, repo, i["identifier"]) if i["status"] == "active" else ""}
            for i in lj(sb, repo, "issue", "list", "--project", "TAL")]


MERGED = None  # no status between the agent's `done` and done


def status(sb: Sandbox, repo: Path, issue: str) -> str:
    return lj(sb, repo, "issue", "get", issue)["status"]


def after_deploy(sb: Sandbox, repo: Path, issue: str) -> dict:
    """The agent's move to `done` already ended the issue: "merged but not done" has no state."""
    return {"commands": [], "outcome": NOT_POSSIBLE, "shows": status(sb, repo, issue)}


def _listed(sb, repo) -> list[dict]:
    return [{"id": i["identifier"], "title": i["title"], "finished": i["status"] in FINISHED}
            for i in lj(sb, repo, "issue", "list", "--project", "TAL")]


def import_findings(sb: Sandbox, repo: Path, path: Path) -> dict:
    """lific's `import` reads GitHub, Linear and Jira, not a checker's run, so the findings go
    through a script (common.findings_by_script)."""
    return common.findings_by_script(
        path, _listed(sb, repo),
        create=lambda title: lific(sb, repo, "issue", "create", "--project", "TAL", "--title", title, "--status", "todo"),
        close=lambda issue: lific(sb, repo, "issue", "update", issue, "--status", "done"),
        list_command="lific issue list --project TAL --json", create_command="lific issue create --title <code fingerprint: message>",
        close_command="lific issue update <id> --status done")


def finding_states(sb: Sandbox, repo: Path) -> dict[str, bool]:
    return common.finding_states(_listed(sb, repo))


def take_record(sb: Sandbox, repo: Path, issues: list[str]) -> list[dict]:
    out = []
    for issue in issues:
        for row in activity(sb, repo, issue):
            found = re.search(r"status: (\w+) → (\w+)$", row["what"])
            if found and "active" in found.groups():
                out.append({"issue": issue, "at": row["at"].isoformat(), "who": row["who"],
                            "event": "take" if found.group(2) == "active" else "end"})
    return sorted(out, key=lambda e: e["at"])


def stop(sb: Sandbox, repo: Path) -> None:
    for key in [k for k in _sessions if k[0].startswith(str(sb.root))]:
        _sessions.pop(key).close()


COMMENTS = re.compile(r"^--- Comments \(\d+\) ---$", re.MULTILINE)
COMMENT = re.compile(r"^\[[^\]\n]+\] (?:\[[^\]\n]+\] )?[^:\n]+: ", re.MULTILINE)


def comment_bodies(answer: str) -> list[str]:
    section = COMMENTS.split(answer, maxsplit=1)
    if len(section) < 2:
        return []
    return [body.strip() for body in COMMENT.split(section[1])[1:]]


def dump(sb: Sandbox, repo: Path, issue: str) -> list[str]:
    return [*comment_bodies(appended(sb, repo, issue)), rewritten(sb, repo, issue)]
