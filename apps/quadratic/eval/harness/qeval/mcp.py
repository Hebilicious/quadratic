"""Every read waits at most READ_LIMIT seconds, a hang guard far above a healthy answer
(beads-mcp may first download its package)."""

from __future__ import annotations

import json
import subprocess
import threading
import time

from qeval.processes import Hang, Owned

READ_LIMIT = 300.0


class Session:
    def __init__(self, cmd: list[str], env: dict, cwd: str, read_limit: float = READ_LIMIT):
        self.process = Owned(cmd, cwd=cwd, env=env, stdin=True, stderr=subprocess.DEVNULL)
        self.read_limit = read_limit
        self.counter = 0
        self.lock = threading.Lock()
        try:
            init = self.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                               "clientInfo": {"name": "quadratic-eval", "version": "0"}})
            self.instructions = (init.get("result") or {}).get("instructions") or ""
            self._send({"jsonrpc": "2.0", "method": "notifications/initialized"}, self.read_limit)
        except BaseException:
            self.process.stop()
            raise

    def _send(self, message: dict, limit: float) -> None:
        self.process.write(json.dumps(message) + "\n", limit)

    def request(self, method: str, params: dict) -> dict:
        with self.lock:
            deadline = time.monotonic() + self.read_limit
            self.counter += 1
            want = self.counter
            self._send({"jsonrpc": "2.0", "id": want, "method": method, "params": params}, self.read_limit)
            while True:
                if time.monotonic() >= deadline:
                    raise Hang(f"the MCP server did not answer {method} in {self.read_limit:.0f} s")
                line = self.process.readline(deadline - time.monotonic())
                if not line:
                    raise RuntimeError(f"MCP server closed during {method}")
                try:
                    message = json.loads(line)
                except ValueError:
                    continue
                if message.get("id") == want:
                    return message

    def tools(self) -> dict:
        return self.request("tools/list", {})["result"]

    def call(self, tool: str, args: dict) -> dict:
        answer = self.request("tools/call", {"name": tool, "arguments": args})
        result = answer.get("result") or {}
        text = "".join(part.get("text", "") for part in result.get("content", []))
        error = bool(result.get("isError")) or "error" in answer
        return {"ok": not error, "text": text or json.dumps(answer.get("error"))}

    def close(self) -> None:
        self.process.close_stdin()
        self.process.stop()
