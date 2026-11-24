from __future__ import annotations

import json
import re
import subprocess
from pathlib import Path
from typing import Callable, Literal

NOT_POSSIBLE = "not possible"
MISSING = "no entry in the history"

Outcome = Literal["ok", "refused", "error"]


def status_of(done: subprocess.CompletedProcess, refused: bool) -> Outcome:
    if done.returncode == 0:
        return "ok"
    return "refused" if refused else "error"


FINGERPRINT = re.compile(r"f:[0-9a-f]{16}")


def findings_by_script(path: Path, listed: list[dict], create: Callable[[str], None],
                       close: Callable[[str], None], list_command: str, create_command: str,
                       close_command: str) -> dict:
    run = json.loads(path.read_text())
    reported = {}
    for finding in run:
        reported.setdefault(FINGERPRINT.search(finding["key"]).group(0), finding)
    known = {m.group(0): row for row in listed if (m := FINGERPRINT.search(row["title"]))}
    commands = [list_command]
    for fingerprint, finding in reported.items():
        if fingerprint not in known:
            message = finding["description"].split("\n", 1)[0]
            create(f"{finding['key']}: {message[:80]}")
            commands.append(create_command)
    for fingerprint, row in known.items():
        if fingerprint not in reported and not row["finished"]:
            close(row["id"])
            commands.append(close_command)
    return {"commands": commands, "outcome": "workaround"}


def finding_states(listed: list[dict]) -> dict[str, bool]:
    return {m.group(0): row["finished"] for row in listed if (m := FINGERPRINT.search(row["title"]))}
