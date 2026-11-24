from __future__ import annotations

import json
from datetime import datetime
from pathlib import Path

from qeval import tally
from qeval.adapters import common
from qeval.adapters.common import MISSING, NOT_POSSIBLE, Outcome, status_of
from qeval.sandbox import BEADS_MCP, Sandbox, fetch, launch_tool

NAME = "beads"
FINISH_WAITS_FOR_REVIEW = False
FINISHED = {"closed"}


def binaries(quadratic: Path) -> dict[str, Path]:
    return {"bd": fetch("bd")}


def bd(sb: Sandbox, cwd: Path, *args: str, env: dict | None = None, check: bool = True, record: bool = False):
    return sb.run(["bd", *args], cwd, extra=env, check=check, record=record)


def bj(sb: Sandbox, cwd: Path, *args: str, env: dict | None = None):
    return json.loads(bd(sb, cwd, *args, "--json", env=env).stdout)


def one(value):
    return value[0] if isinstance(value, list) else value


def setup(sb: Sandbox, repo: Path) -> None:
    bd(sb, repo, "init", record=True)


def seed(sb: Sandbox, repo: Path) -> dict[str, str]:
    ids = {}
    for spec in tally.ISSUES:
        args = ["create", spec.title, "-p", str(spec.priority), "-d", spec.description, "--silent"]
        if spec.acceptance:
            args += ["--acceptance", spec.acceptance]
        if isinstance(spec.assignee, tally.Human):
            args += ["-a", "human"]
        elif isinstance(spec.assignee, tally.Role):  # Beads has no roles: the role becomes a label
            args += ["-l", f"role-{spec.assignee.name}"]
        ids[spec.key] = bd(sb, repo, *args).stdout.strip()
    for spec in tally.ISSUES:
        for target in spec.blocks:
            bd(sb, repo, "dep", "add", ids[target], ids[spec.key])
    return ids


def agent_env(sb: Sandbox, repo: Path, agent: str, distinct: bool) -> dict:
    """The distinct row sets BEADS_ACTOR, which Beads documents for multi-agent work; its
    instructions to agents do not mention it."""
    return {"BEADS_ACTOR": agent} if distinct else {}


def new_issue(sb: Sandbox, repo: Path, title: str, description: str = "base") -> str:
    return bd(sb, repo, "create", title, "-p", "2", "-d", description, "--silent").stdout.strip()


def _issue(sb, cwd, issue, env=None) -> dict:
    return one(bj(sb, cwd, "show", issue, env=env))


def claim(sb: Sandbox, cwd: Path, issue: str, env: dict) -> Outcome:
    done = bd(sb, cwd, "update", issue, "--claim", "--json", env=env, check=False)
    return status_of(done, refused="already claimed" in done.stdout + done.stderr)


def holder(sb: Sandbox, repo: Path, issue: str) -> str:
    return _issue(sb, repo, issue).get("assignee") or ""


def append(sb: Sandbox, cwd: Path, issue: str, env: dict, text: str) -> Outcome:
    return status_of(bd(sb, cwd, "note", issue, text, env=env, check=False), refused=False)


def appended(sb: Sandbox, repo: Path, issue: str) -> str:
    return _issue(sb, repo, issue).get("notes") or ""


def rewrite(sb: Sandbox, cwd: Path, issue: str, env: dict, line: str) -> Outcome:
    text = (_issue(sb, cwd, issue, env).get("description") or "") + "\n" + line
    return status_of(bd(sb, cwd, "update", issue, "-d", text, env=env, check=False), refused=False)


def rewritten(sb: Sandbox, repo: Path, issue: str) -> str:
    return _issue(sb, repo, issue).get("description") or ""


def release(sb, cwd, issue, env):
    bd(sb, cwd, "unclaim", issue, env=env)


def set_priority(sb, repo, issue, priority: int):
    bd(sb, repo, "priority", issue, str(priority))


def finish(sb, cwd, issue, env):
    bd(sb, cwd, "close", issue, "--reason", "tests pass", env=env)


def _at(snapshot_date: str) -> datetime:
    return datetime.fromisoformat(snapshot_date)


def _snapshot_at(sb, repo, issue, moment: datetime) -> dict | None:
    snapshots = bj(sb, repo, "history", issue) or []
    before = [s for s in snapshots if _at(s["CommitDate"]) <= moment]
    return max(before, key=lambda s: _at(s["CommitDate"]))["Issue"] if before else None


def history_answers(sb: Sandbox, repo: Path, x: str, marks: dict) -> dict:
    events = sorted(bj(sb, repo, "history", x, "--events") or [], key=lambda e: e["created_at"])
    claims = [e for e in events if e["event_type"] == "claimed"]
    priority = [e for e in events if e["event_type"] == "updated" and '"priority"' in (e.get("new_value") or "")]
    closes = [e for e in events if e["event_type"] == "closed"]
    m1 = datetime.fromisoformat(marks["m1"])
    x_m1 = _snapshot_at(sb, repo, x, m1) or {}
    issues = bj(sb, repo, "list", "--all", "--flat")
    started = sorted(i["id"] for i in issues
                     if (_snapshot_at(sb, repo, i["id"], m1) or {}).get("status") == "in_progress")
    return {
        "first_claim_by": claims[0]["actor"] if claims else MISSING,
        "first_claim_branch": NOT_POSSIBLE,  # Beads records no git branch
        "priority_changed_by": priority[-1]["actor"] if priority else MISSING,
        "priority_changed_to": str(json.loads(priority[-1]["new_value"])["priority"]) if priority else MISSING,
        "priority_changed_at": priority[-1]["created_at"] if priority else MISSING,
        "finished_by": closes[-1]["actor"] if closes else MISSING,
        "finished_branch": NOT_POSSIBLE,
        "status_at_m1": x_m1.get("status", ""),
        "holder_at_m1": x_m1.get("assignee", ""),
        "started_at_m1": started,
        "_commands": 3 + len(issues),
    }


HUMAN_RECIPE = [
    ["list", "--status", "in_progress", "--flat"],
    ["list", "--assignee", "human", "--flat"],
]


def human_view(sb: Sandbox, repo: Path) -> tuple[int, list[dict]]:
    rows = []
    for args in HUMAN_RECIPE:
        for item in bj(sb, repo, *args):
            rows.append({"id": item["id"], "status": item["status"], "holder": item.get("assignee") or "",
                         "branch": "", "waits_on_human": "--assignee" in args})
    return len(HUMAN_RECIPE), rows


def instructions(sb: Sandbox, repo: Path) -> dict[str, str]:
    agents = (repo / "AGENTS.md").read_text().replace(tally.FILES["AGENTS.md"], "", 1)
    hook = json.loads(bd(sb, repo, "prime", "--hook-json").stdout)
    context = (hook.get("hookSpecificOutput") or {}).get("additionalContext") or json.dumps(hook)
    return {"AGENTS.md sections": agents, "CLAUDE.md": (repo / "CLAUDE.md").read_text(), "SessionStart hook (bd prime)": context}


def mcp_command(sb: Sandbox, repo: Path) -> tuple[list[str], dict]:
    uv = launch_tool("uv")
    return [str(uv), "tool", "run", "--from", BEADS_MCP, "beads-mcp"], {
        "BEADS_PATH": str(sb.bin / "bd"), "BEADS_WORKING_DIR": str(repo), **sb.tool_env()}


def finished(sb: Sandbox, repo: Path, issue: str) -> bool:
    return _issue(sb, repo, issue)["status"] in FINISHED


def board(sb: Sandbox, repo: Path) -> list[dict]:
    return [{"id": i["id"], "title": i["title"], "status": i["status"], "finished": i["status"] in FINISHED,
             "holder": i.get("assignee") or ""} for i in bj(sb, repo, "list", "--all", "--flat")]


def take_record(sb: Sandbox, repo: Path, issues: list[str]) -> list[dict]:
    """A claim of an issue that the same actor already holds is answered as done and records
    no event."""
    out = []
    for issue in issues:
        for e in reversed(bj(sb, repo, "history", issue, "--events") or []):  # newest first
            kind = {"claimed": "take", "unclaimed": "end", "closed": "end"}.get(e["event_type"])
            if kind:
                out.append({"issue": issue, "at": e["created_at"], "who": e["actor"], "event": kind})
    return sorted(out, key=lambda e: e["at"])


MERGED = None  # no status between the agent's close and done


def status(sb: Sandbox, repo: Path, issue: str) -> str:
    return _issue(sb, repo, issue)["status"]


def after_deploy(sb: Sandbox, repo: Path, issue: str) -> dict:
    """The agent's `bd close` already ended the issue, so "merged but not done" has no state in
    the default profile (`bd config set status.custom` could add one: a setting the quickstart
    does not make)."""
    return {"commands": [], "outcome": NOT_POSSIBLE, "shows": status(sb, repo, issue)}


def _listed(sb, repo) -> list[dict]:
    return [{"id": i["id"], "title": i["title"], "finished": i["status"] in FINISHED}
            for i in bj(sb, repo, "list", "--all", "--flat")]


def import_findings(sb: Sandbox, repo: Path, path: Path) -> dict:
    """Beads' `bd import` reads Beads' own JSONL and closes nothing that a run no longer
    reports, so the findings go through a script (common.findings_by_script)."""
    return common.findings_by_script(
        path, _listed(sb, repo),
        create=lambda title: bd(sb, repo, "create", title, "-p", "3", "--silent"),
        close=lambda issue: bd(sb, repo, "close", issue, "--reason", "the checker no longer reports it"),
        list_command="bd list --all --flat --json", create_command="bd create <code fingerprint: message>",
        close_command="bd close <id> --reason ...")


def finding_states(sb: Sandbox, repo: Path) -> dict[str, bool]:
    return common.finding_states(_listed(sb, repo))


def stop(sb: Sandbox, repo: Path) -> None:
    pass  # embedded Dolt: no server process


WRITTEN_FIELDS = ("notes", "close_reason", "description", "acceptance_criteria", "design")


def written_fields(answer: dict) -> list[str]:
    return [answer[field] for field in WRITTEN_FIELDS if isinstance(answer.get(field), str)]


def dump(sb: Sandbox, repo: Path, issue: str) -> list[str]:
    return written_fields(_issue(sb, repo, issue))
