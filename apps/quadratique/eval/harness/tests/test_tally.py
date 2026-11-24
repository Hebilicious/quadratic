import json
import re
import shlex
import unittest
from pathlib import Path

from qeval import tally

SEED_SCRIPT = Path(__file__).resolve().parents[2] / "seed-tally.sh"
TRACKER_LINE = "\nTasks live in Quadratique. Run `quadratique instructions`.\n"


def script_files(script: str) -> dict[str, str]:
    heredocs = re.finditer(r"^cat > (\S+) <<'(\w+)'\n(.*?)^\2$", script, re.MULTILINE | re.DOTALL)
    return {path: body for path, _, body in (match.groups() for match in heredocs)}


def script_issues(script: str) -> dict[str, dict]:
    issues = {}
    for key, arguments in re.findall(r"^(t\d+)=\$\(q create (.*?) \| id\)", script, re.MULTILINE | re.DOTALL):
        words = shlex.split(arguments.replace("\\\n", " "))
        issues[key] = {flag.removeprefix("--"): value for flag, value in zip(words[::2], words[1::2])}
    for blocker, blocked in re.findall(r'^q issue link --issue "\$(t\d+)" --kind blocks --target "\$(t\d+)"', script, re.MULTILINE):
        issues[blocker].setdefault("blocks", []).append(blocked)
    return issues


def assignee_argument(assignee: tally.Assignee | None) -> str | None:
    match assignee:
        case tally.Human():
            return json.dumps({"kind": "human"}, separators=(",", ":"))
        case tally.Role(name):
            return json.dumps({"kind": "agent", "role": name}, separators=(",", ":"))
    return None


def spec_issue(spec: tally.Spec) -> dict:
    issue = {"title": spec.title, "priority": str(spec.priority), "description": spec.description}
    if spec.acceptance:
        issue["acceptance"] = spec.acceptance
    if (assignee := assignee_argument(spec.assignee)) is not None:
        issue["assignee"] = assignee
    if spec.blocks:
        issue["blocks"] = sorted(spec.blocks)
    return issue


class SeedScript(unittest.TestCase):
    def setUp(self):
        self.script = SEED_SCRIPT.read_text()

    def test_the_script_writes_the_harness_files_and_only_its_agents_md_names_the_tracker(self):
        expected = dict(tally.FILES, **{"AGENTS.md": tally.FILES["AGENTS.md"] + TRACKER_LINE})
        self.assertEqual(script_files(self.script), expected)

    def test_the_script_creates_the_harness_issues_and_links(self):
        found = {key: {**issue, "blocks": sorted(issue["blocks"])} if "blocks" in issue else issue
                 for key, issue in script_issues(self.script).items()}
        self.assertEqual(found, {spec.key: spec_issue(spec) for spec in tally.ISSUES})
