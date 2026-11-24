from __future__ import annotations

import json
from pathlib import Path

from qeval import tally
from qeval.adapters.common import MISSING, NOT_POSSIBLE, Outcome, status_of
from qeval.sandbox import QUADRATIC_SRC, Sandbox

NAME = "quadratic"
SKILL = QUADRATIC_SRC / "skill" / "quadratic" / "SKILL.md"
AGENTS_LINE = ("\nWe use Quadratic for issues: every task is an issue there. Use the `quadratic` skill "
               "(`.agents/skills/quadratic/SKILL.md`) for anything about tasks.\n")
# Claude Code sets CLAUDECODE=1 in every command it runs.
AGENT = {"CLAUDECODE": "1"}
FINISH_WAITS_FOR_REVIEW = True
FINISHED = {"in-review", "merged", "done"}


def binaries(quadratic: Path) -> dict[str, Path]:
    return {"quadratic": quadratic}


def q(sb: Sandbox, cwd: Path, *args: str, env: dict | None = None, check: bool = True, record: bool = False):
    return sb.run(["quadratic", *args], cwd, extra=env, check=check, record=record)


def qj(sb: Sandbox, cwd: Path, *args: str, env: dict | None = None) -> dict:
    return json.loads(q(sb, cwd, *args, env=env).stdout)


def setup(sb: Sandbox, repo: Path) -> None:
    q(sb, repo, "init", "--key", "TAL", "--name", "tally", record=True)


def seed(sb: Sandbox, repo: Path) -> dict[str, str]:
    ids = {}
    for spec in tally.ISSUES:
        args = ["create", "--title", spec.title, "--priority", str(spec.priority), "--description", spec.description,
                "--acceptance", spec.acceptance]
        if isinstance(spec.assignee, tally.Human):
            args += ["--assignee", '{"kind":"human"}']
        elif isinstance(spec.assignee, tally.Role):
            args += ["--assignee", json.dumps({"kind": "agent", "role": spec.assignee.name})]
        ids[spec.key] = qj(sb, repo, *args)["issue"]["id"]
    for spec in tally.ISSUES:
        for target in spec.blocks:
            q(sb, repo, "issue", "link", "--issue", ids[spec.key], "--kind", "blocks", "--target", ids[target])
    return ids


def agent_env(sb: Sandbox, repo: Path, agent: str, distinct: bool) -> dict:
    env = dict(AGENT)
    if distinct:
        env["QUADRATIC_ACTOR_FIELDS"] = json.dumps({"session": f"session-{agent}"})
    return env


def new_issue(sb: Sandbox, repo: Path, title: str, description: str = "base") -> str:
    return qj(sb, repo, "create", "--title", title, "--priority", "2", "--description", description,
              "--acceptance", "done")["issue"]["id"]


def _issue(sb, cwd, issue, env=None) -> dict:
    return qj(sb, cwd, "show", "--issue", issue, env=env)["issue"]


def claim(sb: Sandbox, cwd: Path, issue: str, env: dict) -> Outcome:
    done = q(sb, cwd, "claim", "--issue", issue, env=env, check=False)
    return status_of(done, refused='"Held"' in done.stdout + done.stderr)


def holder(sb: Sandbox, repo: Path, issue: str) -> str:
    claim_ = _issue(sb, repo, issue).get("claim") or {}
    return (claim_.get("holder") or {}).get("name", "")


def append(sb: Sandbox, cwd: Path, issue: str, env: dict, text: str) -> Outcome:
    revision = str(_issue(sb, cwd, issue, env)["revision"])
    done = q(sb, cwd, "issue", "update", "--issue", issue, "--revision", revision, "--notes", text, env=env, check=False)
    return status_of(done, refused='"Stale"' in done.stdout)


def appended(sb: Sandbox, repo: Path, issue: str) -> str:
    return "\n".join(note["text"] for note in _issue(sb, repo, issue).get("notes") or [])


def rewrite(sb: Sandbox, cwd: Path, issue: str, env: dict, line: str) -> Outcome:
    current = _issue(sb, cwd, issue, env)
    text = (current.get("description") or "") + "\n" + line
    done = q(sb, cwd, "issue", "update", "--issue", issue, "--revision", str(current["revision"]),
             "--description", text, env=env, check=False)
    return status_of(done, refused='"Stale"' in done.stdout)


def rewritten(sb: Sandbox, repo: Path, issue: str) -> str:
    return _issue(sb, repo, issue).get("description") or ""


def release(sb, cwd, issue, env):
    q(sb, cwd, "release", "--issue", issue, "--reason", "handing over", env=env)


def set_priority(sb, repo, issue, priority: int):
    revision = str(_issue(sb, repo, issue)["revision"])
    q(sb, repo, "issue", "update", "--issue", issue, "--revision", revision, "--priority", str(priority))


def finish(sb, cwd, issue, env):
    q(sb, cwd, "close", "--issue", issue, "--evidence", "tests pass", env=env)


def history_answers(sb: Sandbox, repo: Path, x: str, marks: dict) -> dict:
    changes = list(reversed(qj(sb, repo, "history", "query", "--issue", x)["items"]))
    claims = [c for c in changes if c["operation"] == "issue.claim"]
    priority = [c for c in changes if any(f["field"] == "priority" for f in c["fields"])]
    priority = [c for c in priority if c["operation"] != "issue.create"]
    at_m1 = qj(sb, repo, "issue", "list", "--asOf", marks["m1"])["items"]
    x_at_m1 = next(i for i in at_m1 if i["id"] == x)
    finish_ = [c for c in changes if c["operation"] == "issue.close"]
    return {
        "first_claim_by": claims[0]["author"]["name"] if claims else MISSING,
        "first_claim_branch": claims[0].get("branch") or NOT_POSSIBLE,
        "priority_changed_by": priority[-1]["author"]["name"] if priority else MISSING,
        "priority_changed_to": next(f["after"] for f in priority[-1]["fields"] if f["field"] == "priority") if priority else MISSING,
        "priority_changed_at": priority[-1]["at"] if priority else MISSING,
        "finished_by": finish_[-1]["author"]["name"] if finish_ else MISSING,
        "finished_branch": finish_[-1].get("branch") or NOT_POSSIBLE,
        "status_at_m1": x_at_m1["status"],
        "holder_at_m1": ((x_at_m1.get("claim") or {}).get("holder") or {}).get("name", ""),
        "started_at_m1": sorted(i["id"] for i in at_m1 if i["category"] == "started"),
        "_commands": 2,
    }


HUMAN_RECIPE = [
    ["issue", "list", "--category", "started"],
    ["issue", "list", "--assignee", '{"kind":"human"}', "--category", "unstarted"],
]


def human_view(sb: Sandbox, repo: Path) -> tuple[int, list[dict]]:
    rows = []
    for args in HUMAN_RECIPE:
        for item in qj(sb, repo, *args)["items"]:
            claim_ = item.get("claim") or {}
            rows.append({"id": item["id"], "status": item["status"],
                         "holder": (claim_.get("holder") or {}).get("name", ""),
                         "branch": claim_.get("branch", ""),
                         "waits_on_human": item["status"] == "in-review" or "--assignee" in args})
    return len(HUMAN_RECIPE), rows


def instructions(sb: Sandbox, repo: Path) -> dict[str, str]:
    env = dict(AGENT)
    return {
        "AGENTS.md line": AGENTS_LINE,
        "skill": SKILL.read_text(),
        "quadratic instructions": q(sb, repo, "instructions", env=env).stdout,
    }


def mcp_command(sb: Sandbox, repo: Path) -> tuple[list[str], dict]:
    return ["quadratic", "mcp"], {}


def finished(sb: Sandbox, repo: Path, issue: str) -> bool:
    return _issue(sb, repo, issue)["status"] in FINISHED


def board(sb: Sandbox, repo: Path) -> list[dict]:
    out = []
    for item in qj(sb, repo, "issue", "list", "--limit", "200")["items"]:
        claim_ = item.get("claim") or {}
        out.append({"id": item["id"], "title": item["title"], "status": item["status"],
                    "finished": item["status"] in FINISHED,
                    "holder": (claim_.get("holder") or {}).get("name", "")})
    return out


def take_record(sb: Sandbox, repo: Path, issues: list[str]) -> list[dict]:
    items = qj(sb, repo, "history", "query", "--project", "TAL", "--limit", "200")["items"]
    if len(items) == 200:
        raise RuntimeError("the history has more than 200 entries: page through it")
    out = []
    for change in items:
        for field in change.get("fields") or []:
            if field["field"] == "claim":
                out.append({"issue": change["issue"], "at": change["at"], "who": change["author"]["name"],
                            "event": "take" if field.get("after") else "end"})
    return sorted(out, key=lambda e: e["at"])


MERGED = "merged"


def status(sb: Sandbox, repo: Path, issue: str) -> str:
    return _issue(sb, repo, issue)["status"]


def after_deploy(sb: Sandbox, repo: Path, issue: str) -> dict:
    revision = str(_issue(sb, repo, issue)["revision"])
    q(sb, repo, "issue", "update", "--issue", issue, "--revision", revision, "--status", "done")
    return {"commands": [f"quadratic show --issue {issue}",
                         f"quadratic issue update --issue {issue} --revision {revision} --status done"],
            "outcome": status(sb, repo, issue)}


def import_findings(sb: Sandbox, repo: Path, path: Path) -> dict:
    q(sb, repo, "findings", "import", "--tool", "linter", "--findings", path.read_text(), "--project", "TAL")
    return {"commands": [f"quadratic findings import --tool linter --findings \"$(cat {path.name})\" --project TAL"],
            "outcome": "import"}


def _issues_shown_one_by_one_for_their_source(sb: Sandbox, repo: Path) -> list[dict]:
    listed = qj(sb, repo, "issue", "list", "--limit", "200")["items"]
    return [_issue(sb, repo, item["id"]) for item in listed]


def finding_states(sb: Sandbox, repo: Path) -> dict[str, bool]:
    issues = _issues_shown_one_by_one_for_their_source(sb, repo)
    return {issue["source"]["key"]: issue["status"] in FINISHED for issue in issues if issue.get("source")}


def stop(sb: Sandbox, repo: Path) -> None:
    q(sb, repo, "server", "stop", check=False)


WRITTEN_FIELDS = ("evidence", "description", "acceptance")


def written_fields(answer: dict) -> list[str]:
    notes = [note["text"] for note in answer.get("notes") or []]
    return notes + [answer[field] for field in WRITTEN_FIELDS if isinstance(answer.get(field), str)]


def dump(sb: Sandbox, repo: Path, issue: str) -> list[str]:
    return written_fields(_issue(sb, repo, issue))
