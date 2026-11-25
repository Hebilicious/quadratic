
import os
import shutil
import signal
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest import mock

from qeval import agents, mcp, processes

ENV = {"PATH": "/usr/bin:/bin"}
GUARD = 120


def recording_owner():
    started = []
    real = processes.Owned

    def owned(*args, **kwargs):
        started.append(real(*args, **kwargs))
        return started[-1]

    return started, owned


def in_thread(function):
    box = {}

    def target():
        try:
            box["value"] = function()
        except BaseException as error:
            box["error"] = error

    thread = threading.Thread(target=target, daemon=True)
    thread.start()
    thread.join(GUARD)
    if thread.is_alive():
        raise AssertionError(f"still running after {GUARD} s")
    if "error" in box:
        raise box["error"]
    return box.get("value")


class OwnedProcesses(unittest.TestCase):
    def test_a_command_whose_child_keeps_the_pipe_returns_when_it_ends_and_the_child_is_stopped(self):
        started, owned = recording_owner()
        with mock.patch.object(processes, "Owned", owned):
            done = in_thread(lambda: processes.run(["sh", "-c", "sleep 600 & echo started"], cwd="/", env=ENV))
        self.assertEqual((done.returncode, done.stdout), (0, "started\n"))
        self.assertEqual(processes.group_members(started[0].group), [])

    def test_a_command_that_never_ends_is_stopped_with_its_children_at_its_limit(self):
        started, owned = recording_owner()
        with mock.patch.object(processes, "Owned", owned), self.assertRaises(processes.Hang):
            in_thread(lambda: processes.run(["sh", "-c", "sleep 600 & sleep 600"], cwd="/", env=ENV, timeout=1))
        self.assertEqual(processes.group_members(started[0].group), [])

    def test_a_command_that_never_reads_a_large_input_is_stopped_at_its_limit(self):
        started, owned = recording_owner()
        with mock.patch.object(processes, "Owned", owned), self.assertRaises(processes.Hang):
            in_thread(lambda: processes.run(["sleep", "600"], cwd="/", env=ENV, stdin="x" * 10_000_000,
                                            timeout=0.05))
        self.assertEqual(processes.group_members(started[0].group), [])

    @unittest.skipUnless(shutil.which("setsid"), "needs the setsid command of util-linux, which macOS lacks")
    def test_a_child_that_left_the_group_and_holds_the_pipe_fails_the_cleanup_at_its_deadline(self):
        with tempfile.TemporaryDirectory() as folder:
            left, pid_file = Path(folder) / "left", Path(folder) / "escaped.pid"
            os.mkfifo(left)
            escape = (f"setsid sh -c 'echo $$ > {left}; exec sleep 600' & "
                      f"read escaped < {left}; echo $escaped > {pid_file}; echo started")
            owned = processes.Owned(["sh", "-c", escape], cwd="/", env=ENV)
            try:
                self.assertEqual(owned.wait(GUARD), 0)
                with self.assertRaisesRegex(processes.Hang, "output pipes open past its limit"):
                    in_thread(lambda: owned.stop(time.monotonic()))
            finally:
                if pid_file.exists():
                    os.kill(int(pid_file.read_text()), signal.SIGKILL)

    def test_an_mcp_request_stops_consuming_queued_lines_at_its_deadline(self):
        clock = iter(range(1_000_000))
        queued = mock.Mock()
        queued.readline.return_value = "{}\n"
        session = mcp.Session.__new__(mcp.Session)
        session.process, session.read_limit, session.counter, session.lock = queued, 3, 0, threading.Lock()
        with mock.patch.object(mcp.time, "monotonic", lambda: next(clock)), self.assertRaises(processes.Hang):
            in_thread(lambda: session.request("tools/list", {}))

    def test_an_mcp_server_that_writes_only_other_lines_fails_the_request_at_its_limit(self):
        started, owned = recording_owner()
        chatty = ["sh", "-c", "while true; do echo '{}'; sleep 0.01; done"]
        with mock.patch.object(mcp, "Owned", owned), self.assertRaises(processes.Hang):
            in_thread(lambda: mcp.Session(chatty, ENV, "/", read_limit=1))
        self.assertEqual(processes.group_members(started[0].group), [])

    def test_an_mcp_server_that_never_answers_fails_the_start_and_is_stopped(self):
        started, owned = recording_owner()
        silent = ["sh", "-c", "sleep 600 & cat > /dev/null"]
        with mock.patch.object(mcp, "Owned", owned), self.assertRaises(processes.Hang):
            in_thread(lambda: mcp.Session(silent, ENV, "/", read_limit=1))
        self.assertEqual(processes.group_members(started[0].group), [])

    def test_a_session_that_ends_while_its_child_keeps_the_pipe_is_read_to_the_end(self):
        started, owned = recording_owner()
        session = ["sh", "-c", 'echo \'{"type": "result"}\'; sleep 600 & exit 0']
        with tempfile.TemporaryDirectory() as folder, mock.patch.object(processes, "Owned", owned):
            transcript = Path(folder) / "agent.jsonl"
            ran = in_thread(lambda: agents._drive(session, Path(folder), ENV, transcript))
            self.assertEqual((ran["exit"], ran["timed_out"]), (0, False))
            self.assertIn('"result"', transcript.read_text())
        self.assertEqual(processes.group_members(started[0].group), [])



class HarnessGit(unittest.TestCase):
    def test_a_git_command_of_the_harness_runs_no_hook_of_the_trackers_repository(self):
        from qeval import tally

        with tempfile.TemporaryDirectory() as folder:
            repo, marker = Path(folder) / "repo", Path(folder) / "hook-ran"
            env = {**ENV, "HOME": folder, "GIT_CONFIG_NOSYSTEM": "1"}
            tally.create_repo(repo, env)
            hook = repo / "hooks" / "post-checkout"
            hook.parent.mkdir()
            hook.write_text(f"#!/bin/sh\ntouch {marker}\n")
            hook.chmod(0o755)
            tally.git(repo, "config", "core.hooksPath", str(hook.parent), env=env)
            tally.git(repo, "worktree", "add", "--detach", "-q", str(Path(folder) / "check"), "main", env=env)
            self.assertFalse(marker.exists())

    def test_a_hook_that_the_maintainers_commit_runs_finds_the_trackers_wrapper_first(self):
        from qeval import sandbox, tally

        with tempfile.TemporaryDirectory() as folder:
            marker = Path(folder) / "tracker-ran"
            tracker = Path(folder) / "real-bd"
            tracker.write_text(f'#!/bin/sh\nprintf %s "$QUADRATIQUE_HOME" > {marker}\n')
            tracker.chmod(0o755)
            sb = sandbox.Sandbox(Path(folder) / "beads", "beads", {"bd": tracker}).create()
            repo = sb.root / "tally"
            tally.create_repo(repo, sb.env())
            hook = repo / ".git" / "hooks" / "pre-commit"
            hook.write_text("#!/bin/sh\nbd hooks run pre-commit\n")
            hook.chmod(0o755)
            (repo / "notes.txt").write_text("a change\n")
            tally.commit_all(repo, sb.env(), "A maintainer's commit")
            self.assertEqual(marker.read_text(), str(sb.root / "qhome"))


if __name__ == "__main__":
    unittest.main()
