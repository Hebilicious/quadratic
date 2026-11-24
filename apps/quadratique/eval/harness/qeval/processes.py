from __future__ import annotations

import os
import queue
import signal
import subprocess
import threading
import time
from pathlib import Path

GRACE = 5.0
GROUP_LIMIT = 30.0


class Hang(RuntimeError):
    pass


def group_members(group: int) -> list[int]:
    members = []
    for stat in Path("/proc").glob("[0-9]*/stat"):
        try:
            fields = stat.read_text().rsplit(")", 1)[1].split()
        except (OSError, IndexError):
            continue
        state, process_group = fields[0], int(fields[2])
        if process_group == group and state != "Z":
            members.append(int(stat.parent.name))
    return members


def _signal_group(group: int, signal_number: int) -> None:
    try:
        os.killpg(group, signal_number)
    except ProcessLookupError:
        pass


class Owned:
    def __init__(self, args: list[str], *, cwd: str | Path, env: dict[str, str], stdin: bool = False,
                 stderr=subprocess.PIPE):
        self.args = args
        self.process = subprocess.Popen(
            args, cwd=cwd, env=env, text=True, start_new_session=True,
            stdin=subprocess.PIPE if stdin else subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=stderr)
        self.group = self.process.pid  # a new session: the group's id is the process's
        self._lines: queue.Queue[str | None] = queue.Queue()
        self._errors: list[str] = []
        self._read_streams = [(self.process.stdout, threading.Thread(
            target=self._pump, args=(self.process.stdout, self._lines.put), daemon=True))]
        if stderr is subprocess.PIPE:
            self._read_streams.append((self.process.stderr, threading.Thread(
                target=self._pump, args=(self.process.stderr, self._errors.append), daemon=True)))
        self._readers = [reader for _, reader in self._read_streams]
        self._stopped = threading.Lock()
        self._watcher = threading.Thread(target=self._end_group_on_exit, daemon=True)
        for thread in [*self._readers, self._watcher]:
            thread.start()

    @staticmethod
    def _pump(stream, put) -> None:
        try:
            for line in stream:
                put(line)
        finally:
            stream.close()
            put(None)

    def _end_group_on_exit(self) -> None:
        self.process.wait()
        self._end_group()

    def _end_group(self) -> None:
        with self._stopped:
            if not group_members(self.group):
                return
            _signal_group(self.group, signal.SIGTERM)
            deadline = time.monotonic() + GRACE
            while group_members(self.group) and time.monotonic() < deadline:
                time.sleep(0.05)
            _signal_group(self.group, signal.SIGKILL)
            deadline = time.monotonic() + GROUP_LIMIT
            while group_members(self.group) and time.monotonic() < deadline:
                time.sleep(0.05)

    def write(self, text: str, limit: float) -> None:
        failures: list[BaseException] = []

        def send() -> None:
            try:
                self.process.stdin.write(text)
                self.process.stdin.flush()
            except BaseException as error:
                failures.append(error)

        writer = threading.Thread(target=send, daemon=True)
        writer.start()
        writer.join(max(limit, 0))
        if writer.is_alive():
            raise Hang(f"{self.args[0]} read no input in {limit:.0f} s")
        if failures:
            raise failures[0]

    def close_stdin(self) -> None:
        try:
            self.process.stdin.close()
        except BrokenPipeError:
            pass

    def readline(self, limit: float) -> str:
        try:
            line = self._lines.get(timeout=max(limit, 0))
        except queue.Empty:
            raise Hang(f"{self.args[0]} wrote no line in {limit:.0f} s") from None
        if line is None:
            self._lines.put(None)
            return ""
        return line

    def wait(self, limit: float) -> int:
        try:
            return self.process.wait(timeout=max(limit, 0))
        except subprocess.TimeoutExpired:
            raise Hang(f"{self.args[0]} did not end in {limit:.0f} s") from None

    def stop(self, deadline: float | None = None) -> None:
        if self.process.poll() is None:
            _signal_group(self.group, signal.SIGTERM)
            try:
                self.process.wait(timeout=GRACE)
            except subprocess.TimeoutExpired:
                _signal_group(self.group, signal.SIGKILL)
                self.process.wait()
        self._end_group()
        readers_end = time.monotonic() + GROUP_LIMIT if deadline is None else deadline
        for reader in self._readers:
            reader.join(max(readers_end - time.monotonic(), 0))  # a process that left the group may still hold a pipe
        holding = [stream for stream, reader in self._read_streams if reader.is_alive()]
        self._close_streams(holding)
        survivors = group_members(self.group)
        if holding:
            raise Hang(f"{self.args[0]} left {len(holding)} of its output pipes open past its limit")
        if survivors:
            raise RuntimeError(f"{self.args[0]} was not cleaned up: processes {survivors} still run in its group")

    def _close_streams(self, held: list) -> None:
        for stream in (self.process.stdin, self.process.stdout, self.process.stderr):
            if stream is None or stream.closed or any(stream is each for each in held):
                continue
            try:
                stream.close()
            except (BrokenPipeError, OSError):
                pass

    def output(self) -> str:
        lines = []
        while not self._lines.empty():
            line = self._lines.get()
            if line is not None:
                lines.append(line)
        return "".join(lines)

    def errors(self) -> str:
        return "".join(line for line in self._errors if line is not None)

    def __enter__(self) -> "Owned":
        return self

    def __exit__(self, *exc) -> None:
        self.stop()


def run(args: list[str], *, cwd: str | Path, env: dict[str, str], stdin: str = "",
        timeout: float = 300) -> subprocess.CompletedProcess:
    deadline = time.monotonic() + timeout
    owned = Owned(args, cwd=cwd, env=env, stdin=True)
    try:
        try:
            owned.write(stdin, timeout)
        except BrokenPipeError:
            pass
        owned.close_stdin()
        code = owned.wait(deadline - time.monotonic())
    finally:
        owned.stop(deadline)
    return subprocess.CompletedProcess(args, code, owned.output(), owned.errors())
