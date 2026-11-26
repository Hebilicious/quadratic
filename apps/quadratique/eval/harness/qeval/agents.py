from __future__ import annotations

import json
import re
import shlex
import subprocess
import threading
import time
from pathlib import Path
from typing import Literal

from qeval import processes, sandbox, scripted, tally, tokens
from qeval.adapters.contract import Adapter

PROMPT = "Read its `AGENTS.md` and follow it. Do one task from start to finish, then stop."
MODEL = "claude-opus-5-5"
EFFORT = "medium"
SESSION_LIMIT = 45 * 60
# The user's own instruction files, which --setting-sources alone still loads.
USER_INSTRUCTIONS = [str(Path.home() / ".claude" / "CLAUDE.md"), str(Path.home() / ".agents" / "AGENTS.md")]
DENIED = ["Task", "Workflow", "EnterWorktree", "ExitWorktree", "CronCreate", "RemoteTrigger",
          "ScheduleWakeup", "SendMessage", "PushNotification"]
TRACKER_COMMAND = {t: rf"(^|[\s;&|(]){n}(\s|$)" for t, n in (("quadratique", "quadratique"), ("beads", "bd"), ("lific", "lific"))}
INSTRUCTION_FILES = {"quadratique": [".agents/skills/quadratique/SKILL.md", ".agents/skills/quadratique/planning.md"], "beads": [".agents/skills/beads/SKILL.md", ".beads/README.md"],
                     "lific": []}


def command(worktree: Path, mcp_config: Path | None) -> list[str]:
    cmd = ["claude", "-p", PROMPT, "--model", MODEL, "--effort", EFFORT,
           "--output-format", "stream-json", "--verbose",
           "--setting-sources", "project,local",
           "--settings", json.dumps({"claudeMdExcludes": USER_INSTRUCTIONS, "autoMemoryEnabled": False}),
           "--permission-mode", "bypassPermissions",
           "--disable-slash-commands", "--no-session-persistence",
           "--disallowedTools", ",".join(DENIED),
           "--strict-mcp-config"]
    if mcp_config:
        cmd += ["--mcp-config", str(mcp_config)]
    return cmd


def session_env(sb: sandbox.Sandbox) -> dict[str, str]:
    """The real HOME: Claude Code's sign-in lives there."""
    home = str(Path.home())
    return {"HOME": home, "USER": Path.home().name, "LANG": "C.UTF-8", "TERM": "dumb", "SHELL": "/bin/bash",
            "PATH": f"{sb.bin}:{sandbox.system_path()}", "PROTO_HOME": sandbox.proto_home(), "DO_NOT_TRACK": "1"}


PIPE_LIMIT = 60.0


def _drive(cmd: list[str], cwd: Path, env: dict, out: Path, limit: float = SESSION_LIMIT) -> dict:
    started = time.time()
    deadline = time.monotonic() + limit
    timed_out = False
    with out.open("w") as transcript, out.with_suffix(".stderr").open("w") as errors:
        session = processes.Owned(cmd, cwd=cwd, env=env, stderr=errors)
        exited_at = None
        try:
            while True:
                if time.monotonic() >= deadline:
                    timed_out = True
                    break
                if session.process.poll() is not None:
                    exited_at = exited_at or time.monotonic()
                    if time.monotonic() - exited_at > PIPE_LIMIT:
                        break
                try:
                    line = session.readline(1.0)
                except processes.Hang:
                    continue
                if not line:
                    break
                try:
                    event = json.loads(line)
                except ValueError:
                    continue
                event["_received"] = time.time()
                transcript.write(json.dumps(event) + "\n")
                transcript.flush()
        finally:
            session.stop()
    return {"exit": session.process.returncode, "seconds": round(time.time() - started), "timed_out": timed_out}


def run_tool(adapter: Adapter, root: Path, quadratique_bin: Path) -> dict:
    with scripted.Prepared(adapter, root, quadratique_bin, worktrees=3) as p:
        bases = {agent: tally.git(path, "rev-parse", "HEAD", env=p.sb.env()).strip() for agent, path in p.worktrees.items()}
        mcp = {agent: (path / ".mcp.json" if (path / ".mcp.json").exists() else None) for agent, path in p.worktrees.items()}
        env = session_env(p.sb)
        cmds = {agent: command(path, mcp[agent]) for agent, path in p.worktrees.items()}
        (root / "command.json").write_text(json.dumps({"env": env, "commands": cmds}, indent=2))
        seeded_main = tally.git(p.repo, "rev-parse", "main", env=p.sb.env()).strip()
        (root / SEEDED_MAIN).write_text(json.dumps({"commit": seeded_main}))
        runs: dict[str, dict] = {}
        threads = [threading.Thread(target=lambda a=a, w=w: runs.__setitem__(a, _drive(cmds[a], w, env, root / f"{a}.jsonl")))
                   for a, w in p.worktrees.items()]
        [t.start() for t in threads]
        [t.join() for t in threads]
        (root / "runs.json").write_text(json.dumps(runs))
        return analyze(adapter, p, root)


class Existing:
    def __init__(self, adapter: Adapter, root: Path, quadratique_bin: Path):
        self.adapter = adapter
        self.sb = sandbox.Sandbox(root, adapter.NAME, adapter.binaries(quadratique_bin)).create()
        self.repo = root / "tally"
        self.worktrees = {a: root / a for a in scripted.AGENTS if (root / a).exists()}

    def stop(self) -> None:
        try:
            self.adapter.stop(self.sb, self.repo)
        finally:
            self.sb.remove()


def reanalyze(adapter: Adapter, root: Path, quadratique_bin: Path) -> dict:
    p = Existing(adapter, root, quadratique_bin)
    try:
        return analyze(adapter, p, root)
    finally:
        p.stop()


SEEDED_MAIN = "seeded-main.json"


def comparison_base(p, root: Path) -> dict:
    recorded = root / SEEDED_MAIN
    if recorded.exists():
        return {"commit": json.loads(recorded.read_text())["commit"], "from": "the seeded main, recorded when the run started"}
    now = tally.git(p.repo, "rev-parse", "main", env=p.sb.env()).strip()
    print(f"{root}: no {SEEDED_MAIN}, a run from before it was recorded; comparing with main as it is now", flush=True)
    return {"commit": now, "from": f"main at analysis: the run has no {SEEDED_MAIN}"}


def analyze(adapter: Adapter, p, root: Path) -> dict:
    runs = json.loads((root / "runs.json").read_text())
    comparison = comparison_base(p, root)
    base = comparison["commit"]
    result = {"sessions": {}, "board": adapter.board(p.sb, p.repo), "comparison_base": comparison}
    mcp = mcp_definitions(adapter, p) if (p.repo / ".mcp.json").exists() else None
    for agent, path in p.worktrees.items():
        transcript = read_transcript(root / f"{agent}.jsonl")
        calls = tracker_calls(transcript, adapter.NAME, path, mcp)
        start = start_context(transcript, path, adapter.NAME, mcp)
        result["sessions"][agent] = {
            **runs[agent],
            "usage": transcript["usage"], "cost_usd": transcript["cost_usd"], "turns": transcript["turns"],
            "tracker": tracker_tokens(transcript, calls, adapter.NAME, start),
            "calls": [{k: c[k] for k in ("t", "kind", "name", "ok", "refused", "input", "output")} for c in calls],
            "completed": completed(adapter, p, agent, path, base, calls, result["board"]),
        }
    record = adapter.take_record(p.sb, p.repo, [row["id"] for row in result["board"]])
    (root / "take-record.json").write_text(json.dumps(record, indent=2, default=str))
    result["double_claims"] = double_claims(record, list(p.worktrees))
    result["double_claims_in_transcripts"] = double_claims_in_transcripts(result["sessions"], adapter)
    result["lost_updates"] = lost_updates(adapter, p, result["sessions"])
    return result


def mcp_definitions(adapter: Adapter, p) -> dict:
    from qeval.mcp import Session

    cmd, extra = adapter.mcp_command(p.sb, p.repo)
    session = Session(cmd, p.sb.env(extra), str(p.repo))
    try:
        listed = session.tools()
        return {"tools": listed["tools"], "tokens": tokens.count_tools(listed), "instructions": session.instructions}
    finally:
        session.close()


def read_transcript(path: Path) -> dict:
    uses, results, init, final, hooks, first = [], {}, {}, {}, [], None
    for line in path.read_text().splitlines():
        event = json.loads(line)
        if event.get("type") == "system" and event.get("subtype") == "init":
            init = event
        elif event.get("type") == "system" and event.get("subtype") == "hook_response":
            hooks.append(hook_context(event))
        elif event.get("type") == "assistant":
            if first is None:
                first = event["message"]["usage"]
            for block in event["message"].get("content", []):
                if block.get("type") == "tool_use":
                    uses.append({"id": block["id"], "name": block["name"], "input": block.get("input", {}),
                                 "t": event["_received"]})
        elif event.get("type") == "user":
            content = event.get("message", {}).get("content", [])
            for block in content if isinstance(content, list) else []:
                if block.get("type") == "tool_result":
                    text, refs = block.get("content"), []
                    if isinstance(text, list):
                        # ToolSearch answers with tool_reference blocks: Claude Code then puts
                        # those tools' whole definitions into the context.
                        refs = [part["tool_name"] for part in text if isinstance(part, dict) and part.get("type") == "tool_reference"]
                        text = "".join(part.get("text", "") for part in text if isinstance(part, dict))
                    results[block["tool_use_id"]] = {"text": text or "", "error": bool(block.get("is_error")), "refs": refs}
        elif event.get("type") == "result":
            final = event
    for use in uses:
        use["result"] = results.get(use["id"], {"text": "", "error": True, "refs": []})
    first = first or {}
    return {"uses": uses, "init": init, "usage": final.get("usage", {}), "cost_usd": final.get("total_cost_usd"),
            "turns": final.get("num_turns"), "hooks": hooks,
            "first_request_input": sum(first.get(k, 0) for k in ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"))}


def hook_context(event: dict) -> str:
    try:
        answer = json.loads(event.get("stdout") or "{}")
    except ValueError:
        return event.get("stdout") or ""
    return (answer.get("hookSpecificOutput") or {}).get("additionalContext") or ""


def classify(use: dict, tool: str) -> Literal["call", "schemas"] | None:
    if use["name"].startswith(f"mcp__{tool}__"):
        return "call"
    if use["name"] == "ToolSearch" and (tool in json.dumps(use["input"]) or any(
            r.startswith(f"mcp__{tool}__") for r in use["result"].get("refs", []))):
        return "schemas"
    text = use["input"].get("command", "") if use["name"] == "Bash" else use["input"].get("file_path", "")
    if use["name"] == "Bash" and re.search(TRACKER_COMMAND[tool], text):
        return "call"
    return None


def _ok(use: dict, tool: str) -> bool:
    text = use["result"]["text"]
    return not use["result"]["error"] and not (tool == "lific" and text.startswith("Error"))


def instruction_texts(worktree: Path, tool: str) -> dict[str, str]:
    texts = {}
    agents_md = worktree / "AGENTS.md"
    if agents_md.exists():
        part = agents_md.read_text().replace(tally.FILES["AGENTS.md"], "", 1).strip()
        if part:
            texts["AGENTS.md"] = part
    if (worktree / "CLAUDE.md").exists():
        texts["CLAUDE.md"] = (worktree / "CLAUDE.md").read_text().strip()
    for f in INSTRUCTION_FILES[tool]:
        if (worktree / f).exists():
            texts[f] = (worktree / f).read_text().strip()
    return texts


def start_context(transcript: dict, worktree: Path, tool: str, mcp: dict | None) -> dict[str, int]:
    """Claude Code loads CLAUDE.md, and loads AGENTS.md only where the project has no CLAUDE.md
    (its built-in agents-md plugin, default "claude-md-or-agents-md"). It puts the whole MCP tool
    definitions in context only when the session could not defer them (no ToolSearch in its tools)."""
    texts = instruction_texts(worktree, tool)
    parts = {}
    if (worktree / "CLAUDE.md").exists():
        parts["CLAUDE.md"] = tokens.count(texts["CLAUDE.md"])
    elif "AGENTS.md" in texts:
        parts["AGENTS.md"] = tokens.count(texts["AGENTS.md"])
    for i, hook in enumerate(transcript["hooks"]):
        parts[f"SessionStart hook {i + 1}"] = tokens.count(hook)
    names = [n for n in transcript["init"].get("tools", []) if n.startswith(f"mcp__{tool}__")]
    if names and mcp:
        parts["MCP server instructions"] = tokens.count(mcp["instructions"])
        if "ToolSearch" in transcript["init"].get("tools", []):
            parts["MCP tool names (definitions deferred)"] = tokens.count("\n".join(names))
        else:
            parts["MCP tool definitions"] = mcp["tokens"]
    return parts


QUADRATIQUE_KEYS = {"issue", "items", "text", "_tag", "kind", "key", "created"}


def quadratique_answers(output: str) -> list[dict]:
    decoder, found, i = json.JSONDecoder(), [], 0
    while (i := output.find("{", i)) != -1:
        if i == 0 or output[i - 1] == "\n":
            try:
                value, end = decoder.raw_decode(output, i)
            except ValueError:
                i += 1
                continue
            if isinstance(value, dict) and QUADRATIQUE_KEYS & value.keys():
                found.append({"value": value, "text": output[i:end]})
                i = end
                continue
        i += 1
    return found


COMMAND_WORD = {"quadratique": "quadratique", "beads": "bd", "lific": "lific"}
INSTRUCTIONS_COMMAND = {"quadratique": ("instructions",), "beads": ("prime",), "lific": ()}
NEUTRAL = {"cd", "export", "set", "true", ":"}
FILTERS = {"head", "tail", "grep", "jq", "sed", "cut", "sort", "uniq", "less", "cat"}


SCRIPT_FIRST = {"grep", "sed", "jq"}
SCRIPT_OPTIONS = {"-e", "-f", "--expression", "--file", "--regexp", "--from-file"}
VALUE_OPTIONS = {
    "head": {"-n": 1, "-c": 1}, "tail": {"-n": 1, "-c": 1}, "cut": {"-d": 1, "-f": 1, "-c": 1, "-b": 1},
    "sort": {"-k": 1, "-t": 1}, "uniq": {"-f": 1, "-s": 1, "-w": 1},
    "grep": {"-e": 1, "-f": 1, "-m": 1, "-A": 1, "-B": 1, "-C": 1}, "sed": {"-e": 1, "-f": 1},
    "jq": {"--arg": 2, "--argjson": 2, "-f": 1, "--from-file": 1},
}


FLAG_LETTERS = {
    "grep": "inEvcFlwxoqshHLr", "head": "qv", "tail": "qv", "sort": "nrufbhVs", "uniq": "cdui",
    "cut": "s", "sed": "nEr", "jq": "rceSjnsMCa", "cat": "nAbestvET", "less": "RSFX",
}
LONG_FLAGS = {"--ignore-case", "--invert-match", "--count", "--line-number", "--fixed-strings",
              "--extended-regexp", "--raw-output", "--compact-output", "--slurp", "--null-input", "--numeric-sort",
              "--reverse", "--unique", "--quiet", "--silent"}


def _plain_flag(name: str, word: str) -> bool:
    if word in LONG_FLAGS or (word.startswith("--") and "=" in word):
        return True
    cluster = word[1:]
    return word.startswith("-") and not word.startswith("--") and bool(cluster) and (
        cluster.isdigit() or set(cluster) <= set(FLAG_LETTERS.get(name, "")))


def reads_own_input(words: list[str]) -> bool:
    name = _command_name(words)
    arguments = words[words.index(name) + 1:]
    operands, skip, script_given, options_end = [], 0, False, False
    for word in arguments:
        if skip:
            skip -= 1
            continue
        if options_end or word == "-" or not word.startswith("-"):
            operands.append(word)
        elif word == "--":
            options_end = True
        elif word in VALUE_OPTIONS.get(name, {}):
            script_given |= word in SCRIPT_OPTIONS
            skip = VALUE_OPTIONS[name][word]
        elif not _plain_flag(name, word):
            return True
    script_operands = 1 if name in SCRIPT_FIRST and not script_given else 0
    return len(operands) > script_operands


def _command_name(words: list[str]) -> str:
    rest = [w for w in words if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*=.*", w)]
    return rest[0] if rest else ""


def shell_line(call: dict, tool: str) -> dict:
    word = COMMAND_WORD[tool]
    tracker, certain, instructions = [], True, False
    pipeline_head = None
    for operator, words in pipeline(call["input"].get("command", "")):
        name = _command_name(words)
        if operator != "|":
            pipeline_head = name
        if name == word:
            tracker.append(" ".join(words))
            after = words[words.index(word) + 1:]
            wanted = INSTRUCTIONS_COMMAND[tool]
            instructions |= bool(wanted) and tuple(after[:len(wanted)]) == wanted
        elif name in NEUTRAL and operator != "|":
            continue
        elif operator == "|" and pipeline_head == word and name in FILTERS and not reads_own_input(words):
            continue
        else:
            certain = False
    return {"tracker": "\n".join(tracker), "certain": certain, "instructions": instructions}


def tracker_calls(transcript: dict, tool: str, worktree: Path, mcp: dict | None = None) -> list[dict]:
    texts = instruction_texts(worktree, tool)
    loaded_at_start = {"CLAUDE.md"} if (worktree / "CLAUDE.md").exists() else {"AGENTS.md"}
    calls = []
    for use in transcript["uses"]:
        kind = classify(use, tool)
        output = use["result"]["text"]
        read = []
        if use["name"] == "Read":
            path = use["input"].get("file_path", "")
            read = [label for label in texts if path.endswith("/" + label)]
        else:
            read = [label for label, text in texts.items() if text and text in output]
        for label in read:
            output = output.replace(texts[label], "")
        if read:
            calls.append({"t": use["t"], "kind": "read", "name": use["name"], "ok": True, "refused": False,
                          "input": {"files": read}, "output": "\n".join(texts[l] for l in read),
                          "again": [l for l in read if l in loaded_at_start]})
        if kind == "schemas":
            fetched = [r for r in use["result"].get("refs", []) if r.startswith(f"mcp__{tool}__")]
            definitions = [d for d in (mcp or {}).get("tools", []) if f"mcp__{tool}__{d['name']}" in fetched]
            calls.append({"t": use["t"], "kind": "schemas", "name": use["name"], "ok": True, "refused": False,
                          "input": use["input"], "output": tokens.compact(definitions), "fetched": fetched})
        elif kind == "call":
            whole_input = tokens.compact(use["input"])
            if use["name"] == "Bash":
                line = shell_line(use, tool)
                answers = quadratique_answers(output) if tool == "quadratique" else []
                lower_output = output if line["certain"] else "\n".join(a["text"] for a in answers)
                lower_input = line["tracker"]
                instructions_upper = output if line["instructions"] else ""
                alone = line["certain"] and line["tracker"].count("\n") == 0
                instructions_lower = (output if alone else "\n".join(
                    a["text"] for a in answers if str(a["value"].get("text", "")).startswith("You are"))
                ) if line["instructions"] else ""
            else:
                lower_output, lower_input = output, whole_input
                instructions_upper = instructions_lower = ""
            calls.append({"t": use["t"], "kind": "call", "name": use["name"], "ok": _ok(use, tool),
                          "refused": REFUSED[tool] in use["result"]["text"],
                          "input": use["input"], "output": output,
                          "tokens": {"input": (tokens.count(lower_input), tokens.count(whole_input)),
                                     "output": (tokens.count(lower_output), tokens.count(output)),
                                     "instructions": (tokens.count(instructions_lower), tokens.count(instructions_upper))},
                          "runs_instructions": bool(instructions_upper)})
    return calls


def _bounds(pairs) -> dict:
    pairs = list(pairs)
    return {"lower": sum(p[0] for p in pairs), "upper": sum(p[1] for p in pairs)}


def tracker_tokens(transcript: dict, calls: list[dict], tool: str, start: dict[str, int]) -> dict:
    tracker_calls_ = [c for c in calls if c["kind"] == "call"]
    reads = [c for c in calls if c["kind"] == "read"]
    fetches = [c for c in calls if c["kind"] == "schemas"]
    return {
        "start": start,
        "start_total": sum(start.values()),
        "calls": len(tracker_calls_),
        "call_input": _bounds(c["tokens"]["input"] for c in tracker_calls_),
        "call_output": _bounds(c["tokens"]["output"] for c in tracker_calls_),
        "instruction_reads": sum(tokens.count(c["output"]) for c in reads),
        "reads": [{"files": c["input"]["files"], "again": c["again"]} for c in reads],
        "schema_fetches": sum(tokens.count(tokens.compact(c["input"])) + tokens.count(c["output"]) for c in fetches),
        "fetched_tools": sorted({t for c in fetches for t in c["fetched"]}),
        "instructions_command_runs": sum(c["runs_instructions"] for c in tracker_calls_),
        "instructions_command": _bounds(c["tokens"]["instructions"] for c in tracker_calls_),
        "first_request_input": transcript["first_request_input"],
    }


def segments(command_text: str) -> list[list[str]]:
    return [words for _, words in pipeline(command_text)]


# A redirection that names a file descriptor (2>&1, >&2, &>file) is part of its word, not a
# command separator.
REDIRECTION = re.compile(r"(\d*[<>])&(\d|-)|&>")


def pipeline(command_text: str) -> list[tuple[str, list[str]]]:
    text = REDIRECTION.sub(lambda m: m.group(0).replace("&", "\x00"), command_text)
    lexer = shlex.shlex(text, posix=True, punctuation_chars=";&|\n")
    lexer.whitespace = " \t\r"
    lexer.whitespace_split = True
    try:
        tokens_ = list(lexer)
    except ValueError:
        return [("", command_text.split())]
    out, current, operator = [], [], ""
    for token in tokens_:
        if token and set(token) <= set(";&|\n"):
            if current:
                out.append((operator, current))
            current, operator = [], token.strip("\n") or ";"
        else:
            current.append(token.replace("\x00", "&"))
    if current:
        out.append((operator, current))
    return out


ISSUE = {"quadratique": r"TAL-\d+", "beads": r"tally-[a-z0-9]+(?:\.\d+)?", "lific": r"TAL-\d+"}


def _issues_in(call: dict, tool: str, verbs: tuple[str, ...], mcp_status: tuple[str, ...]) -> list[str]:
    if call["name"] == "Bash":
        found = []
        command_word = COMMAND_WORD[tool]
        for words in segments(call["input"].get("command", "")):
            if command_word not in words or not any(v in words for v in verbs):
                continue
            if tool == "lific" and "update" not in words:
                continue
            found += [w for w in words if re.fullmatch(ISSUE[tool], w)]
        return found
    if call["name"].endswith("update_issue") and call["input"].get("status") in mcp_status:
        return [call["input"].get("identifier", "")]
    return []


# A shell line's exit code can hide a refused take.
REFUSED = {"quadratique": '"Held"', "beads": "already claimed", "lific": "Error:"}

VERBS = {
    "finish": {"quadratique": ("finish", "close"), "beads": ("close",), "lific": ("done",)},
    "take": {"quadratique": ("claim",), "beads": ("--claim",), "lific": ("active",)},
    "give_back": {"quadratique": ("release",), "beads": ("unclaim",), "lific": ("todo", "backlog")},
}


def issues_by(calls: list[dict], tool: str, what: str) -> list[tuple[float, str]]:
    verbs = VERBS[what][tool]
    out = []
    for call in calls:
        if call["kind"] == "call" and call["ok"] and not (what == "take" and call.get("refused")):
            for issue in _issues_in(call, tool, verbs, verbs):
                out.append((call["t"], issue))
    return out


def task_of(issue: str, board: list[dict]) -> tally.Spec | None:
    title = next((row["title"] for row in board if row["id"] == issue), None)
    return next((spec for spec in tally.ISSUES if spec.title == title), None)


def code_about(issues: list[str], board: list[dict], changed: list[str], kept: list[str]) -> dict[str, list[str]]:
    out = {}
    for issue in issues:
        spec = task_of(issue, board)
        files = sorted(set(spec.paths) & set(changed)) if spec else []
        tests_changed = any(path.startswith("tests/") and path.endswith(".py") for path in set(changed) & set(kept))
        if files and (tests_changed or not spec.tested):
            out[issue] = files
    return out


SKIPPED = re.compile(r"^(test\w+) \(.*\) \.\.\. skipped", re.MULTILINE)
PASSED = re.compile(r"^(test\w+) \(.*\) \.\.\. ok$", re.MULTILINE)
RAN = re.compile(r"^Ran (\d+) tests?", re.MULTILINE)


def test_verdict(code: int, report: str, enables: tuple[str, ...]) -> dict:
    ran = int(m.group(1)) if (m := RAN.search(report)) else 0
    skipped = SKIPPED.findall(report)
    still_skipped = sorted(set(enables) & set(skipped))
    not_passed = sorted(set(enables) - set(PASSED.findall(report)))
    return {"ran": ran, "skipped": skipped, "still_skipped": still_skipped, "required_not_passed": not_passed,
            "passed": code == 0 and ran > len(skipped) and not still_skipped and not not_passed}


def completed(adapter: Adapter, p, agent: str, worktree: Path, base: str, calls: list[dict],
              board: list[dict]) -> dict:
    env = p.sb.env()
    finished_ids = sorted({issue for _, issue in issues_by(calls, adapter.NAME, "finish")})
    tracker_ok = [i for i in finished_ids if adapter.finished(p.sb, p.repo, i)]
    changed = tally.git(p.repo, "diff", "--name-only", base, agent, env=env).splitlines()
    kept = tally.git(p.repo, "diff", "--name-only", "--diff-filter=d", base, agent, env=env).splitlines()
    about = code_about(tracker_ok, board, changed, kept)
    candidates = list(about)
    tests = None
    if candidates:
        check = p.sb.root / f"check-{agent}"
        tally.git(p.repo, "worktree", "add", "--detach", "-q", str(check), agent, env=env)
        try:
            run = processes.run(["python3", "-m", "unittest", "-v"], cwd=check, env=env, timeout=600)
            tests = test_verdict(run.returncode, run.stderr, task_of(candidates[0], board).enables)
        finally:
            tally.git(p.repo, "worktree", "remove", "--force", str(check), env=env)
    task = candidates[0] if tests and tests["passed"] else None
    return {"finished_in_tracker": tracker_ok, "finish_calls": finished_ids, "changed": changed,
            "about": about, "tests": tests, "task": task, "completed": task is not None}


def double_claims(record: list[dict], sessions: list[str]) -> dict:
    def session_of(who: str) -> str | None:
        found = [s for s in sessions if s in who]
        return found[0] if len(found) == 1 else None

    takers = {e["who"] for e in record if e["event"] == "take"}
    if not takers:
        return {"established": False, "doubles": [], "why": "the record holds no take"}
    unnamed = sorted(w for w in takers if session_of(w) is None)
    if unnamed:
        return {"established": False, "doubles": [],
                "why": f"the record names its takers {', '.join(repr(w) for w in unnamed)}, which tell no session apart"}
    holders: dict[str, set] = {}
    doubles = []
    for e in record:
        held = holders.setdefault(e["issue"], set())
        if e["event"] == "end":
            held.clear()
            continue
        session = session_of(e["who"])
        if held - {session}:
            doubles.append({"issue": e["issue"], "second": session, "held_by": sorted(held - {session}), "at": e["at"]})
        held.add(session)
    return {"established": True, "doubles": doubles, "why": ""}


def took(call: dict, issue: str, tool: str) -> bool:
    if call["name"] != "Bash":
        return call["ok"]
    output = call.get("output", "")
    if tool == "quadratique":
        return '"Held"' not in output and any(
            (a["value"].get("issue") or {}).get("id") == issue and (a["value"]["issue"].get("claim"))
            for a in quadratique_answers(output))
    if tool == "beads":
        return f"Updated issue: {issue}" in output and "already claimed" not in output
    return call["ok"]


def double_claims_in_transcripts(sessions: dict, adapter: Adapter) -> list[dict]:
    tool = adapter.NAME
    events = []
    for agent, s in sessions.items():
        calls = s["calls"]
        events += [(t, agent, "take", issue) for t, issue in issues_by(calls, tool, "take")
                   if took(next(c for c in calls if c["t"] == t), issue, tool)]
        ends = ("give_back",) if adapter.FINISH_WAITS_FOR_REVIEW else ("give_back", "finish")
        for what in ends:
            events += [(t, agent, what, issue) for t, issue in issues_by(calls, tool, what)]
    events.sort()
    holders: dict[str, set] = {}
    doubles = []
    for t, agent, what, issue in events:
        held = holders.setdefault(issue, set())
        if what == "take":
            if held - {agent}:
                doubles.append({"issue": issue, "second": agent, "held_by": sorted(held - {agent}), "t": t})
            held.add(agent)
        else:
            held.discard(agent)
    return doubles


TEXT_FLAGS = {"--notes", "--evidence", "--reason", "--append-notes", "-r"}
LITERAL_MINIMUM = 20


def shell_text(word: str) -> list[str | None]:
    parts, literal, i = [], "", 0
    while i < len(word):
        char = word[i]
        if char == "\\" and word[i + 1:i + 2] in ("$", "`"):
            literal, i = literal + word[i + 1], i + 2
            continue
        end = None
        if word.startswith("$(", i):
            depth, end = 0, i + 1
            while end < len(word):
                depth += {"(": 1, ")": -1}.get(word[end], 0)
                if depth == 0:
                    break
                end += 1
        elif word.startswith("${", i):
            end = word.find("}", i)
        elif char == "$" and re.match(r"[A-Za-z_0-9?#@*!$-]", word[i + 1:i + 2]):
            end = i + (len(m.group(0)) if (m := re.match(r"[A-Za-z_][A-Za-z_0-9]*", word[i + 1:])) else 1)
        elif char == "`":
            end = word.find("`", i + 1)
        if end is None:
            literal, i = literal + char, i + 1
            continue
        parts += [literal, None] if literal else [None]
        literal, i = "", (len(word) if end == -1 else end + 1)
    return parts + ([literal] if literal else [])


def _segment_texts(words: list[str], tool: str) -> list[tuple[str, list[str | None]]]:
    issues = [w for w in words if re.fullmatch(ISSUE[tool], w)]
    if not issues or COMMAND_WORD[tool] not in words:
        return []
    out = []
    for flag, value in zip(words, words[1:]):
        if flag in TEXT_FLAGS:
            out.append((issues[0], shell_text(value)))
    for word in words:
        flag, _, value = word.partition("=")
        if flag in TEXT_FLAGS and value:
            out.append((issues[0], shell_text(value)))
    if tool == "beads" and len(words) >= 4 and words[:2] == ["bd", "note"]:
        out.append((words[2], shell_text(" ".join(words[3:]))))
    return out


def written_texts(call: dict, tool: str) -> list[tuple[str, list[str | None]]]:
    return [(issue, parts) for issue, parts, _ in confirmed_texts(call, tool)]


def _norm(text: str) -> str:
    return re.sub(r"\s+", " ", text).strip()


def still_stored(parts: list[str | None], fields: list[str]) -> bool | None:
    stored = [_norm(field) for field in fields]
    if None not in parts:
        text = _norm("".join(p for p in parts if p))
        if not text:
            return None
        whole = re.compile(rf"(?<!\w){re.escape(text)}(?!\w)")
        return any(whole.search(field) for field in stored)
    literals = [_norm(p) for p in parts if p]
    if sum(map(len, literals)) < LITERAL_MINIMUM:
        return None
    body = ".*?".join(re.escape(_norm(p)) if p else "" for p in parts)
    before = r"(?<!\w)" if parts[0] else ""
    after = r"(?!\w)" if parts[-1] else ""
    pattern = re.compile(before + body + after, re.DOTALL)
    return any(pattern.search(field) for field in stored)


STDOUT_REDIRECTION = re.compile(r"^(1?>>?|&>>?)")
ANSWERS_NO_ISSUE = (("instructions",), ("ready",), ("list",), ("issue", "list"), ("history",), ("project",))


def confirmed_texts(call: dict, tool: str) -> list[tuple[str, list[str | None], bool]]:
    if call["name"] != "Bash":
        args = call["input"]
        if call["name"].endswith("add_comment"):
            return [(args.get("identifier", ""), [args.get("content", "")], call["ok"])]
        if call["name"].endswith("update_issue") and args.get("evidence"):
            return [(args.get("identifier", ""), [args["evidence"]], call["ok"])]
        return []
    commands = pipeline(call["input"].get("command", ""))
    trackers = [i for i, (_, words) in enumerate(commands) if COMMAND_WORD[tool] in words]

    def failure_reaches_the_exit_code(i: int) -> bool:
        after = [operator for operator, _ in commands[i + 1:]]
        return not after or (after[0] == "&&" and set(after) <= {"&&", "|"})

    def answers_no_issue(words: list[str]) -> bool:
        after = words[words.index(COMMAND_WORD[tool]) + 1:]
        return any(tuple(after[:len(prefix)]) == prefix for prefix in ANSWERS_NO_ISSUE)

    def answers_alone(i: int) -> bool:
        words = commands[i][1]
        if tool != "quadratique" or any(STDOUT_REDIRECTION.match(w) for w in words):
            return False
        if not all(answers_no_issue(commands[j][1]) for j in trackers if j != i):
            return False
        lines = call.get("output", "").splitlines()
        return any(line.startswith('{"issue":') for line in lines) and not any(line.startswith('{"_tag"') for line in lines)

    out = []
    for i in trackers:
        confirmed = (call["ok"] and failure_reaches_the_exit_code(i)) or answers_alone(i)
        out += [(issue, parts, confirmed) for issue, parts in _segment_texts(commands[i][1], tool)]
    return out


def lost_updates(adapter: Adapter, p, sessions: dict) -> dict:
    made, checked, lost, unchecked = 0, 0, [], []
    for agent, s in sessions.items():
        for call in s["calls"]:
            if call["kind"] != "call" or not call["ok"]:
                continue
            for issue, parts, confirmed in confirmed_texts(call, adapter.NAME):
                made += 1
                text = "".join(part if part else "…" for part in parts)
                if not confirmed:
                    unchecked.append({"agent": agent, "issue": issue, "text": text[:80], "why": "not confirmed"})
                    continue
                present = still_stored(parts, adapter.dump(p.sb, p.repo, issue))
                if present is None:
                    unchecked.append({"agent": agent, "issue": issue, "text": text[:80], "why": "too short"})
                    continue
                checked += 1
                if not present:
                    lost.append({"agent": agent, "issue": issue, "text": text[:80]})
    return {"made": made, "checked": checked, "lost": lost, "unchecked": unchecked}
