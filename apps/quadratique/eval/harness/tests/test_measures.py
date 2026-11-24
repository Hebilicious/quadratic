import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from qeval import agents, report, tokens, written
from qeval.adapters import lific, quadratique


def call(name, input, ok=True, t=0.0):
    return {"t": t, "kind": "call", "name": name, "ok": ok, "input": input}


class FilesWritten(unittest.TestCase):
    def test_the_files_of_one_skill_folder_count_as_one_file(self):
        quadratique = ["A\t.agents/skills/quadratique/SKILL.md", "A\t.agents/skills/quadratique/planning.md", "M\tAGENTS.md"]
        beads = ["A\t.agents/skills/beads/SKILL.md", "A\t.agents/skills/beads/agents/openai.yaml",
                 "A\t.claude/settings.json", "A\t.codex/config.toml", "A\t.claude/skills/beads/SKILL.md",
                 "A\t.codex/skills/beads/SKILL.md", "A\t.codex/skills/other/SKILL.md"]
        lific = ["A\t.mcp.json", "M\tAGENTS.md"]
        self.assertEqual(written.counted_files(quadratique), 2)
        self.assertEqual(written.counted_files(beads), 6)
        self.assertEqual(written.counted_files(lific), 2)

    def test_a_rename_stored_by_an_older_run_counts_its_source_and_its_destination(self):
        stored = ["R100\t.agents/skills/old/notes.md\t.agents/skills/x/notes.md", "A\t.agents/skills/x/SKILL.md"]
        self.assertEqual(written.counted_files(stored), 2)

    def test_loose_files_count_each_and_a_skill_with_its_skill_file_counts_once(self):
        files = ["M\tAGENTS.md", "A\t.agents/skills/README.md",
                 "A\t.agents/skills/q/SKILL.md", "A\t.agents/skills/q/planning.md", "A\t.agents/skills/q/résumé.md",
                 "A\t.claude/skills/loose/a.md", "A\t.claude/skills/loose/b.md"]
        self.assertEqual(written.counted_files(files), 5)


class LificActivity(unittest.TestCase):
    def test_an_agent_line_and_an_operator_line_both_parse(self):
        agent = lific.ACTIVITY.match("- [2030-01-15 16:39:11] Claude Code (agent) via mcp — TAL-1 status: todo → active")
        operator = lific.ACTIVITY.match("- [2030-01-15 16:53:35] system via cli — TAL-9 priority: none → urgent")
        self.assertEqual((agent["who"], agent["what"]), ("Claude Code", "TAL-1 status: todo → active"))
        self.assertEqual((operator["who"], operator["what"]), ("system", "TAL-9 priority: none → urgent"))


class TrackerCalls(unittest.TestCase):
    def test_each_trackers_take_and_finish_are_read_from_the_call(self):
        cases = [
            ("quadratique", call("Bash", {"command": "cd /w && quadratique claim --issue TAL-3"}), "take", ["TAL-3"]),
            ("quadratique", call("Bash", {"command": 'quadratique close --issue TAL-3 --evidence "ok"'}), "finish", ["TAL-3"]),
            ("beads", call("Bash", {"command": "bd update tally-x1y --claim --json"}), "take", ["tally-x1y"]),
            ("beads", call("Bash", {"command": 'bd close tally-x1y --reason "done"'}), "finish", ["tally-x1y"]),
            ("lific", call("mcp__lific__update_issue", {"identifier": "TAL-2", "status": "done"}), "finish", ["TAL-2"]),
            ("lific", call("Bash", {"command": "lific issue update TAL-2 --status active"}), "take", ["TAL-2"]),
        ]
        for tool, c, what, expected in cases:
            with self.subTest(tool=tool, what=what):
                self.assertEqual([i for _, i in agents.issues_by([c], tool, what)], expected)

    def test_a_refused_call_takes_nothing(self):
        refused = call("Bash", {"command": "quadratique claim --issue TAL-3"}, ok=False)
        self.assertEqual(agents.issues_by([refused], "quadratique", "take"), [])

    def test_a_take_refused_inside_a_longer_shell_line_takes_nothing(self):
        held = dict(call("Bash", {"command": "quadratique claim --issue TAL-1; echo; ls"}), refused=True)
        self.assertEqual(agents.issues_by([held], "quadratique", "take"), [])

    def test_a_read_of_the_issue_is_not_a_take(self):
        show = call("Bash", {"command": "bd show tally-x1y --json"})
        self.assertEqual(agents.issues_by([show], "beads", "take"), [])


def take(issue, who, at, event="take"):
    return {"issue": issue, "who": who, "at": at, "event": event}


class DoubleClaims(unittest.TestCase):
    SESSIONS = ["agent-a", "agent-b", "agent-c"]

    def test_a_recorded_take_while_another_session_holds_the_issue_is_a_double_claim(self):
        record = [take("TAL-1", "claude-code@agent-a", "1"), take("TAL-1", "claude-code@agent-b", "2")]
        found = agents.double_claims(record, self.SESSIONS)
        self.assertEqual((found["established"], [d["second"] for d in found["doubles"]]), (True, ["agent-b"]))

    def test_a_take_after_the_record_ends_the_claim_is_not(self):
        record = [take("TAL-1", "claude-code@agent-a", "1"), take("TAL-1", "claude-code@agent-a", "2", "end"),
                  take("TAL-1", "claude-code@agent-b", "3")]
        self.assertEqual(agents.double_claims(record, self.SESSIONS)["doubles"], [])

    def test_a_record_that_names_every_taker_the_same_cannot_establish_the_row(self):
        found = agents.double_claims([take("tally-1", "maintainer", "1")], self.SESSIONS)
        self.assertEqual((found["established"], found["doubles"]), (False, []))

    def test_a_record_with_no_take_cannot_establish_the_row(self):
        found = agents.double_claims([], self.SESSIONS)
        self.assertEqual((found["established"], found["doubles"]), (False, []))

    def test_in_the_transcripts_a_quadratique_close_leaves_the_issue_held(self):
        claimed = call("Bash", {"command": "quadratique claim --issue TAL-1"}, t=1)
        claimed["output"] = '{"issue": {"id": "TAL-1", "claim": {"branch": "agent-a"}}}'
        closed = call("Bash", {"command": "quadratique close --issue TAL-1 --evidence x"}, t=2)
        second = dict(claimed, t=3)
        sessions = {"agent-a": {"calls": [claimed, closed]}, "agent-b": {"calls": [second]}}
        self.assertEqual([d["second"] for d in agents.double_claims_in_transcripts(sessions, quadratique)], ["agent-b"])


class Completed(unittest.TestCase):
    BOARD = [{"id": "TAL-1", "title": "Count the words of a text"}, {"id": "TAL-8", "title": "Add --version to the command line"}]

    def test_a_branch_counts_only_when_it_changes_a_file_the_task_is_about(self):
        noise = ["README.md", "tally/__pycache__/core.cpython-314.pyc", "tests/test_core.py"]
        self.assertEqual(agents.code_about(["TAL-1"], self.BOARD, noise, noise), {})
        self.assertEqual(agents.code_about(["TAL-1"], self.BOARD, noise + ["tally/core.py"], noise + ["tally/core.py"]), {"TAL-1": ["tally/core.py"]})

    def test_a_suite_with_only_skipped_tests_or_the_tasks_test_still_skipped_does_not_pass(self):
        skipped = "test_counts_each_word (tests.test_core.CountWords.test_counts_each_word) ... skipped 'not yet'\n\nRan 1 test in 0.000s\n\nOK (skipped=1)\n"
        self.assertFalse(agents.test_verdict(0, skipped, ())["passed"])
        other = "test_version (tests.test_cli.Version.test_version) ... ok\n" + skipped.replace("Ran 1 test", "Ran 2 tests")
        self.assertTrue(agents.test_verdict(0, other, ())["passed"])
        self.assertFalse(agents.test_verdict(0, other, ("test_counts_each_word",))["passed"])

    def test_a_task_whose_required_test_was_deleted_does_not_pass(self):
        deleted = "test_version (tests.test_cli.Version.test_version) ... ok\n\nRan 1 test in 0.000s\n\nOK\n"
        self.assertFalse(agents.test_verdict(0, deleted, ("test_counts_each_word",))["passed"])
        enabled = "test_counts_each_word (tests.test_core.CountWords.test_counts_each_word) ... ok\n" + deleted.replace("Ran 1 test", "Ran 2 tests")
        self.assertTrue(agents.test_verdict(0, enabled, ("test_counts_each_word",))["passed"])

    def test_a_tested_task_counts_only_when_the_branch_also_changes_a_test(self):
        self.assertEqual(agents.code_about(["TAL-1"], self.BOARD, ["tally/core.py"], ["tally/core.py"]), {})
        compiled = ["tally/core.py", "tests/__pycache__/test_core.cpython-314.pyc"]
        self.assertEqual(agents.code_about(["TAL-1"], self.BOARD, compiled, compiled), {})
        deleted = ["tally/core.py", "tests/test_core.py"]
        self.assertEqual(agents.code_about(["TAL-1"], self.BOARD, deleted, ["tally/core.py"]), {})


class WrittenTexts(unittest.TestCase):
    def test_notes_evidence_and_comments_are_found_with_their_issue(self):
        self.assertEqual(agents.written_texts(call("Bash", {"command": 'quadratique close --issue TAL-1 --evidence "tests pass"'}), "quadratique"),
                         [("TAL-1", ["tests pass"])])
        self.assertEqual(agents.written_texts(call("Bash", {"command": "bd note tally-1 found a bug"}), "beads"),
                         [("tally-1", ["found a bug"])])
        self.assertEqual(agents.written_texts(call("mcp__lific__add_comment", {"identifier": "TAL-4", "content": "hi"}), "lific"),
                         [("TAL-4", ["hi"])])

    def test_a_text_with_a_semicolon_or_a_newline_inside_quotes_is_read_whole(self):
        line = 'git commit -qm x\nquadratique close --issue TAL-1 --evidence "a; b\nc" && git log'
        self.assertEqual(agents.written_texts(call("Bash", {"command": line}), "quadratique"), [("TAL-1", ["a; b\nc"])])

    def test_a_flag_written_with_an_equals_sign_is_read(self):
        line = 'bd close tally-1 --reason="tests pass" && git status'
        self.assertEqual(agents.written_texts(call("Bash", {"command": line}), "beads"), [("tally-1", ["tests pass"])])

    def test_the_parts_the_shell_fills_are_marked_and_an_escaped_dollar_is_literal(self):
        self.assertEqual(agents.shell_text("Commit $(git rev-parse --short HEAD) on agent-b"), ["Commit ", None, " on agent-b"])
        self.assertEqual(agents.shell_text("sha $sha, exit ${code}"), ["sha ", None, ", exit ", None])
        self.assertEqual(agents.shell_text("printed \\`tally\\` and \\$?"), ["printed `tally` and $?"])


class LostUpdates(unittest.TestCase):
    EVIDENCE = "Commit 5e6f7a8 on branch agent-a. The test passes; python3 -m unittest prints OK."

    def test_a_text_counts_as_stored_only_when_all_of_it_is(self):
        self.assertTrue(agents.still_stored([self.EVIDENCE], ["status", self.EVIDENCE]))
        self.assertFalse(agents.still_stored([self.EVIDENCE], [self.EVIDENCE[:60] + " (cut)"]))

    def test_a_text_the_shell_built_is_checked_by_its_literal_parts(self):
        parts = agents.shell_text("Commit $(git rev-parse HEAD) on branch agent-b. Tests pass.")
        self.assertTrue(agents.still_stored(parts, ["Commit 1a2b3c4 on branch agent-b. Tests pass."]))
        self.assertFalse(agents.still_stored(parts, ["Commit 1a2b3c4 on branch agent-c."]))
        self.assertIsNone(agents.still_stored(agents.shell_text("$(cat out.txt)"), ["anything"]))

    def test_a_short_literal_text_is_checked_against_whole_words_of_one_field(self):
        self.assertTrue(agents.still_stored(["hi"], ["status", "hi"]))
        self.assertFalse(agents.still_stored(["hi"], ["this", "high"]))

    def test_a_write_counts_as_confirmed_only_by_its_own_result(self):
        def confirmed(line, tool, output=""):
            return [ok for _, _, ok in agents.confirmed_texts(dict(call("Bash", {"command": line}), output=output), tool)]

        self.assertEqual(confirmed('bd note tally-1 "found a bug"', "beads"), [True])
        self.assertEqual(confirmed('git commit -qm x && bd note tally-1 "found a bug" && git log | head -1', "beads"), [True])
        self.assertEqual(confirmed('bd note tally-1 "found a bug" || true', "beads"), [False])
        self.assertEqual(confirmed("quadratique close --issue TAL-1 --evidence x | head -c 40", "quadratique",
                                   '{"issue":{"id":"TAL-1","status":"in-revi'), [True])
        self.assertEqual(confirmed('quadratique close --issue TAL-1 --evidence x; echo done', "quadratique",
                                   '{"_tag": "Stale"}\ndone\n'), [False])

    def test_a_redirected_write_is_not_confirmed_by_a_later_shows_answer(self):
        line = 'quadratique issue update --issue TAL-1 --revision 3 --notes "found it" > /dev/null 2>&1; quadratique show --issue TAL-1'
        written = agents.confirmed_texts(dict(call("Bash", {"command": line}), output='{"issue":{"id":"TAL-1"}}\n'), "quadratique")
        self.assertEqual(written, [("TAL-1", ["found it"], False)])
        beside = 'quadratique close --issue TAL-1 --evidence "done" | head -c 40; quadratique instructions | grep review'
        answered = agents.confirmed_texts(dict(call("Bash", {"command": beside}), output='{"issue":{"id":"TAL-1"\n'), "quadratique")
        self.assertEqual(answered, [("TAL-1", ["done"], True)])

    def test_a_built_text_matches_only_whole_words(self):
        parts = agents.shell_text("pass $(date)")
        self.assertFalse(agents.still_stored(parts + [" on the branch agent-b"], ["bypass Monday on the branch agent-b"]))
        self.assertTrue(agents.still_stored(parts + [" on the branch agent-b"], ["pass Monday on the branch agent-b"]))

    def test_each_trackers_stored_fields_are_its_written_texts_one_by_one(self):
        from qeval.adapters import beads, quadratique

        lific_answer = ("TAL-1 — Count the words\nStatus: done\n\nCount the words, as the checker says.\n\n"
                        "--- Comments (2) ---\n[2030-01-15 17:21:37] [verification] claude-code-human (Claude Code): Commit 1a2b3c4.\n"
                        "- five tests pass\n[2030-01-15 17:30:00] sam: Thanks\n")
        self.assertEqual(lific.comment_bodies(lific_answer), ["Commit 1a2b3c4.\n- five tests pass", "Thanks"])
        self.assertEqual(quadratique.written_fields({"id": "TAL-1", "title": "Count", "evidence": "e", "notes": [{"text": "n"}]}),
                         ["n", "e"])
        self.assertEqual(beads.written_fields({"id": "tally-1", "title": "Count", "notes": "n", "close_reason": "r"}), ["n", "r"])


class ContextAccounting(unittest.TestCase):
    def setUp(self):
        import shutil
        from pathlib import Path

        from qeval import tally
        self.root = Path(__file__).resolve().parents[1] / ".cache" / "test-worktree"
        shutil.rmtree(self.root, ignore_errors=True)
        self.root.mkdir(parents=True)
        (self.root / "AGENTS.md").write_text(tally.FILES["AGENTS.md"] + "\nUse the tracker.\n")
        self.addCleanup(shutil.rmtree, self.root, True)

    def transcript(self, uses=(), tools=(), hooks=()):
        return {"uses": list(uses), "init": {"tools": list(tools)}, "hooks": list(hooks), "first_request_input": 0}

    def test_agents_md_counts_only_where_there_is_no_claude_md(self):
        self.assertEqual(list(agents.start_context(self.transcript(), self.root, "lific", None)), ["AGENTS.md"])
        (self.root / "CLAUDE.md").write_text("Use bd.\n")
        self.assertEqual(list(agents.start_context(self.transcript(), self.root, "beads", None)), ["CLAUDE.md"])

    def test_deferred_mcp_tools_count_their_names_and_a_toolsearch_fetch_counts_the_definitions(self):
        mcp = {"tools": [{"name": "get_issue", "description": "x" * 400}, {"name": "list_issues", "description": "y"}],
               "tokens": 999, "instructions": "Lific is an issue tracker."}
        tools = ["Bash", "ToolSearch", "mcp__lific__get_issue", "mcp__lific__list_issues"]
        start = agents.start_context(self.transcript(tools=tools), self.root, "lific", mcp)
        self.assertNotIn("MCP tool definitions", start)
        self.assertIn("MCP tool names (definitions deferred)", start)
        fetch = {"name": "ToolSearch", "input": {"query": "select:mcp__lific__get_issue"}, "t": 1,
                 "result": {"text": "", "error": False, "refs": ["mcp__lific__get_issue"]}}
        [c] = agents.tracker_calls(self.transcript([fetch], tools), "lific", self.root, mcp)
        self.assertEqual((c["kind"], c["fetched"]), ("schemas", ["mcp__lific__get_issue"]))
        self.assertIn("x" * 400, c["output"])
        self.assertNotIn("list_issues", c["output"])

    def test_instruction_text_inside_a_tracker_call_counts_once_as_a_read(self):
        use = {"name": "Bash", "input": {"command": "cat AGENTS.md; lific issue list"}, "t": 1,
               "result": {"text": (self.root / "AGENTS.md").read_text() + "TAL-1 todo", "error": False, "refs": []}}
        read, call = agents.tracker_calls(self.transcript([use]), "lific", self.root)
        self.assertEqual((read["kind"], read["input"]["files"]), ("read", ["AGENTS.md"]))
        self.assertNotIn("Use the tracker.", call["output"])

    def bash(self, command, output):
        return {"name": "Bash", "input": {"command": command}, "t": 1,
                "result": {"text": output, "error": False, "refs": []}}

    def bounds(self, tool, *uses, mcp=None):
        transcript = self.transcript(uses)
        calls = agents.tracker_calls(transcript, tool, self.root, mcp)
        return agents.tracker_tokens(transcript, calls, tool, {})

    def test_other_tracker_commands_on_the_instructions_line_stay_out_of_its_lower_bound(self):
        reference, ready = '{"text": "You are agent@w (an agent)."}', '{"items": [{"id": "TAL-1"}]}'
        counted = self.bounds("quadratique", self.bash("quadratique instructions; quadratique ready", reference + "\n" + ready))
        self.assertEqual(counted["instructions_command"], {"lower": tokens.count(reference), "upper": tokens.count(reference + "\n" + ready)})

    def test_a_toolsearch_fetch_counts_its_input_too(self):
        query = {"query": "select:mcp__lific__get_issue", "max_results": 5}
        fetch = {"name": "ToolSearch", "input": query, "t": 1,
                 "result": {"text": "", "error": False, "refs": ["mcp__lific__get_issue"]}}
        mcp = {"tools": [{"name": "get_issue"}], "tokens": 0, "instructions": ""}
        counted = self.bounds("lific", fetch, mcp=mcp)["schema_fetches"]
        self.assertEqual(counted, tokens.count(tokens.compact(query)) + tokens.count('[{"name":"get_issue"}]'))

    def test_a_tracker_answer_cut_by_head_counts_whole_in_both_bounds(self):
        cut = '{"issue": {"id": "TAL-1", "title": "Count the'
        counted = self.bounds("quadratique", self.bash("quadratique show --issue TAL-1 2>&1 | head -3", cut))["call_output"]
        self.assertEqual(counted, {"lower": tokens.count(cut), "upper": tokens.count(cut)})

    def test_a_filter_that_reads_its_own_file_takes_the_line_out_of_the_lower_bound(self):
        listing = "# tally\n\nCounts the words of a text file.\n"
        counted = self.bounds("lific", self.bash("lific issue list | cat README.md", listing))["call_output"]
        self.assertEqual(counted["lower"], 0)
        grepped = self.bounds("quadratique", self.bash("quadratique list | grep -n TAL tally/core.py", listing))["call_output"]
        self.assertEqual(grepped["lower"], 0)
        kept = self.bounds("quadratique", self.bash("quadratique list | grep -n TAL | head -n 2", "1:TAL-1\n"))["call_output"]
        self.assertEqual(kept["lower"], tokens.count("1:TAL-1\n"))

    def test_a_filter_that_reads_a_file_after_the_end_of_its_options_takes_the_line_out_of_the_lower_bound(self):
        listing = "-other-file: TAL-1 is mentioned here\n"
        counted = self.bounds("quadratique", self.bash("quadratique list | grep TAL -- -other-file", listing))["call_output"]
        self.assertEqual(counted["lower"], 0)
        unknown = self.bounds("quadratique", self.bash("quadratique list | grep --max-count 3 TAL", listing))["call_output"]
        self.assertEqual(unknown["lower"], 0)

    def test_a_line_that_also_does_other_work_counts_whole_above_and_only_the_trackers_part_below(self):
        answer, listing = '{"issue": {"id": "TAL-1"}}', "def count_words(text):\n    return {}\n"
        quadratique = self.bounds("quadratique", self.bash("quadratique claim --issue TAL-1 && cat tally/core.py", answer + "\n" + listing))
        beads = self.bounds("beads", self.bash("bd update tally-1 --claim && cat tally/core.py", "Updated tally-1\n" + listing))
        self.assertEqual(quadratique["call_input"]["lower"], tokens.count("quadratique claim --issue TAL-1"))
        self.assertEqual(beads["call_input"]["lower"], tokens.count("bd update tally-1 --claim"))
        self.assertEqual((quadratique["call_output"]["lower"], quadratique["call_output"]["upper"]),
                         (tokens.count(answer), tokens.count(answer + "\n" + listing)))
        self.assertEqual((beads["call_output"]["lower"], beads["call_output"]["upper"]),
                         (0, tokens.count("Updated tally-1\n" + listing)))


class FindingsWorkaround(unittest.TestCase):
    def test_the_script_creates_each_new_finding_once_and_closes_only_the_gone_ones(self):
        from pathlib import Path

        from qeval.adapters import common

        fixtures = Path(__file__).resolve().parents[1] / "fixtures"
        created, closed = [], []
        run = lambda path, listed: common.findings_by_script(path, listed, created.append, closed.append, "list", "create", "close")
        run(fixtures / "findings-1.json", [])
        listed = [{"id": f"x-{i}", "title": title, "finished": False} for i, title in enumerate(created)]
        run(fixtures / "findings-2.json", listed)
        self.assertEqual(len(created), 2)
        self.assertEqual([listed[i]["title"] for i in range(2) if listed[i]["id"] in closed], [t for t in created if "f:a420556f407006e2" in t])


    def test_an_import_is_right_only_when_the_gone_findings_issue_is_finished_and_the_other_stays_open(self):
        from types import SimpleNamespace

        from qeval import scripted

        def importing(second_run_finishes_the_gone_one: bool):
            runs = []

            def import_findings(sb, repo, path):
                runs.append(path.name)
                return {"outcome": "import", "commands": [f"import {path.name}"]}

            def finding_states(sb, repo):
                gone_finished = len(runs) == 2 and second_run_finishes_the_gone_one
                return {"D1 f:a420556f407006e2": gone_finished, "D2 f:49f3eaf2628a1f35": False}

            adapter = SimpleNamespace(import_findings=import_findings, finding_states=finding_states)
            return scripted.findings(SimpleNamespace(adapter=adapter, sb=None, repo=None))["score"]

        self.assertEqual(importing(True), "right")
        self.assertEqual(importing(False), "wrong")


class DeployRow(unittest.TestCase):
    def test_a_deploy_that_was_not_reached_is_not_scored(self):
        row = next(r for r in report.rows({"agents": {}, "scripted": {"merge": {"quadratique": [
            {"merge": {}, "deploy": {"commands": [], "outcome": "not reached: the merge was not shown"}}]}}})
                   if r["measure"].startswith("Moving the issue on"))
        self.assertEqual(row["values"]["quadratique"], [report.UNMEASURED])


class WrittenFromGit(unittest.TestCase):
    ENV = {**os.environ, "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_NOSYSTEM": "1",
           "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.invalid",
           "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.invalid"}

    def written_by(self, before: dict[str, str], setup) -> list[str]:
        with tempfile.TemporaryDirectory() as folder:
            repo = Path(folder)

            def git(*args):
                subprocess.run(["git", *args], cwd=repo, env=self.ENV, check=True, capture_output=True)

            git("init", "-q", "-b", "main")
            for name, text in before.items():
                (repo / name).write_text(text)
            git("add", "-A")
            git("commit", "-q", "--allow-empty", "-m", "start")
            setup(repo, git)
            git("add", "-A")
            git("commit", "-q", "-m", "setup")
            return sorted(written.name_status(repo, "HEAD~1", "HEAD", self.ENV))

    def test_a_path_git_would_quote_arrives_as_written_and_counts_in_its_skill(self):
        def setup(repo, git):
            skill = repo / ".agents/skills/example"
            skill.mkdir(parents=True)
            (skill / "SKILL.md").write_text("skill\n")
            (skill / "résumé.md").write_text("more\n")

        files = self.written_by({}, setup)

        self.assertEqual(files, ["A\t.agents/skills/example/SKILL.md", "A\t.agents/skills/example/résumé.md"])
        self.assertEqual(written.counted_files(files), 1)

    def test_a_rename_into_a_skill_lists_its_source_as_deleted(self):
        def setup(repo, git):
            (repo / ".agents/skills/x").mkdir(parents=True)
            git("mv", "notes.md", ".agents/skills/x/notes.md")
            (repo / ".agents/skills/x/SKILL.md").write_text("skill\n")

        files = self.written_by({"notes.md": "a note long enough for git to see a rename\n" * 5}, setup)

        self.assertEqual(files, ["A\t.agents/skills/x/SKILL.md", "A\t.agents/skills/x/notes.md", "D\tnotes.md"])
        self.assertEqual(written.counted_files(files), 2)


class FilesWrittenRow(unittest.TestCase):
    def test_the_row_counts_the_stored_files_under_the_rule_not_a_stored_count(self):
        old_run = {"file_count": 3, "files": ["A\t.agents/skills/beads/SKILL.md", "A\t.agents/skills/beads/agents/openai.yaml",
                                             "M\tAGENTS.md"]}
        row = next(r for r in report.rows({"agents": {}, "scripted": {"setup": {"beads": [old_run]}}})
                   if r["measure"] == "Files written into the repository")
        self.assertEqual(row["values"]["beads"], [2])


class Labels(unittest.TestCase):
    def row(self, better, quadratique, lific, beads):
        return {"better": better, "values": {"quadratique": quadratique, "lific": lific, "beads": beads}}

    def test_a_row_with_a_missing_repeat_is_not_won(self):
        self.assertEqual(report.verdict(self.row("lower", [0, None], [22], [8])), "not won: quadratique not measured")
        failed = self.row("lower", [0], [22, report.UNMEASURED], [8])
        self.assertEqual(report.verdict(failed), "not won: lific not measured")
        self.assertEqual(report.verdict(self.row("lower", [30], [22, report.UNMEASURED], [8])), "lost to lific and beads")

    def test_a_tracker_whose_measurement_is_missing_is_not_excluded_as_by_design(self):
        unreached = self.row("lower", [1], [report.UNMEASURED], [2])
        self.assertEqual(report.verdict(unreached), "not won: lific not measured")
        by_design = self.row("lower", [1], [None], [2])
        self.assertEqual(report.verdict(by_design), "won; lific not compared (no value)")

    def test_a_row_is_won_only_when_no_run_overlaps_and_ranges_count_whole(self):
        self.assertEqual(report.verdict(self.row("lower", [0, 0, 0], [22, 22, 23], [2, 2, 8])), "won")
        self.assertEqual(report.verdict(self.row("lower", [(1698, 1900), (2363, 2500)], [(2213, 2213), (2660, 2660)], [(4161, 4799)])), "tied with lific")
        self.assertEqual(report.verdict(self.row("lower", [2], [5], [1])), "lost to beads")
        self.assertEqual(report.verdict(self.row("lower", [0], [None], [None])), "lific and beads not compared (no value)")


if __name__ == "__main__":
    unittest.main()


class Adapters(unittest.TestCase):
    def test_every_adapter_offers_the_whole_contract(self):
        from qeval.__main__ import ADAPTERS
        from qeval.adapters.contract import Adapter

        for name, adapter in ADAPTERS.items():
            with self.subTest(name):
                self.assertIsInstance(adapter, Adapter)


class ReportArguments(unittest.TestCase):
    def refusal(self, *argv: str) -> str:
        from qeval.__main__ import main

        with self.assertRaises(SystemExit) as stopped:
            main(["report", *argv])
        return str(stopped.exception.code)

    def test_a_report_with_no_run_to_read_names_the_flags_to_pass(self):
        message = self.refusal()
        for flag in ("--run", "--from", "--out"):
            self.assertIn(flag, message)

    def test_a_run_folder_without_results_says_so(self):
        with tempfile.TemporaryDirectory() as empty:
            self.assertIn("results.json does not exist", self.refusal("--run", empty))

    def test_an_unreadable_results_file_names_its_path_and_the_cause(self):
        with tempfile.TemporaryDirectory() as run:
            (Path(run) / "results.json").mkdir()
            message = self.refusal("--run", run)
        self.assertIn(f"cannot read {run}/results.json", message)
        self.assertIn("directory", message)

    def test_from_without_out_names_out(self):
        self.assertIn("--from needs --out", self.refusal("--from", "all=/nowhere"))

    def test_out_without_from_names_from(self):
        self.assertIn("--out needs at least one --from", self.refusal("--out", "/nowhere"))

    def test_run_with_from_or_out_is_refused(self):
        self.assertIn("not both", self.refusal("--run", "/nowhere", "--from", "all=/nowhere"))
        self.assertIn("not both", self.refusal("--run", "/nowhere", "--out", "/nowhere"))


class LaunchTools(unittest.TestCase):
    def test_a_tool_missing_from_the_launching_path_is_named(self):
        from unittest import mock

        from qeval import sandbox

        with mock.patch.object(sandbox, "LAUNCH_PATH", "/nonexistent"), self.assertRaises(SystemExit) as stopped:
            sandbox.launch_tool("uv")
        self.assertIn("no `uv` on the PATH that started the harness", str(stopped.exception.code))
