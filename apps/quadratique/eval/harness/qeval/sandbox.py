"""Hooks and MCP entries written by a tracker call its binaries by name, so the wrapper folder
goes first on PATH and catches them too.
"""

from __future__ import annotations

import hashlib
import os
import shutil
import subprocess
import tarfile
import time
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

from qeval import processes

HARNESS = Path(__file__).resolve().parents[1]
QUADRATIQUE_SRC = HARNESS.parents[1]
CACHE = HARNESS / ".cache"
RUNS = Path(os.environ.get("QEVAL_RUNS", Path.home() / ".cache" / "quadratique-eval" / "runs"))

RELEASES = {
    "bd": {
        "version": "1.3.4",
        "url": "https://github.com/gastownhall/beads/releases/download/v1.3.4/beads_1.3.4_linux_amd64.tar.gz",
        "sums": "https://github.com/gastownhall/beads/releases/download/v1.3.4/checksums.txt",
        "sha256": "175c26904bde3ff9168d2282ff6b56f17b17f60c22fa5acb3dacc9fb3465303a",
        "member": "bd",
    },
    "lific": {
        "version": "2.11.1",
        "url": "https://github.com/VoidNullable/lific/releases/download/v2.11.1/lific-linux-x86_64",
        "sums": "https://github.com/VoidNullable/lific/releases/download/v2.11.1/sha256sums.txt",
        "sha256": "87b20621ce7e075db1548200291372f6d2ca8255fe3d78a555a99b6c31180aed",
        "member": None,
    },
}
BEADS_MCP = "beads-mcp==1.3.4"

MAINTAINER = ("maintainer", "maintainer@example.invalid")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def _download(url: str, target: Path) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    request = urllib.request.Request(url, headers={"User-Agent": "quadratique-eval"})
    with urllib.request.urlopen(request, timeout=300) as answer, target.open("wb") as out:
        shutil.copyfileobj(answer, out)


def fetch(name: str) -> Path:
    pin = RELEASES[name]
    folder = CACHE / "bin" / f"{name}-{pin['version']}"
    binary = folder / name
    if binary.exists():
        return binary
    download = CACHE / "dl" / name / pin["url"].rsplit("/", 1)[1]
    sums = download.with_name("published-sums.txt")
    if not download.exists() or sha256(download) != pin["sha256"]:
        _download(pin["url"], download)
    _download(pin["sums"], sums)
    actual = sha256(download)
    published = {line.split()[1]: line.split()[0] for line in sums.read_text().splitlines() if line.strip()}
    if actual != pin["sha256"] or published.get(download.name) != pin["sha256"]:
        raise SystemExit(f"checksum mismatch for {download}: {actual}")
    folder.mkdir(parents=True, exist_ok=True)
    if pin["member"]:
        with tarfile.open(download) as archive:
            member = archive.getmember(pin["member"])
            with archive.extractfile(member) as source, binary.open("wb") as out:
                shutil.copyfileobj(source, out)
    else:
        shutil.copyfile(download, binary)
    binary.chmod(0o755)
    return binary


# Its own target folder: `quadratique-install:install` builds another binary at target/release/quadratique.
QUADRATIQUE_TARGET = QUADRATIQUE_SRC / "target" / "qeval"


def build_quadratique() -> Path:
    subprocess.run(["cargo", "build", "--release", "-p", "quadratique", "--target-dir", str(QUADRATIQUE_TARGET)],
                   cwd=QUADRATIQUE_SRC, check=True)
    return QUADRATIQUE_TARGET / "release" / "quadratique"


# The PATH of the process that started the harness, before any sandbox narrows it.
LAUNCH_PATH = os.environ.get("PATH", "")
# Commands that sessions and setups call by name: their folders on LAUNCH_PATH join the narrow PATH.
NAMED_TOOLS = ("git", "python3", "uv", "claude")


def proto_home() -> str:
    """Where proto keeps its tools: its shims need it once HOME is replaced."""
    return os.environ.get("PROTO_HOME") or str(Path.home() / ".proto")


def launch_tool(name: str) -> Path:
    found = shutil.which(name, path=LAUNCH_PATH)
    if not found:
        raise SystemExit(f"qeval: no `{name}` on the PATH that started the harness. Install it, or put its folder "
                         "on PATH, and run again.")
    return Path(found)


def system_path() -> str:
    folders = [str(Path(found).parent) for name in NAMED_TOOLS if (found := shutil.which(name, path=LAUNCH_PATH))]
    folders += ["/usr/local/bin", "/usr/bin", "/bin"]
    return ":".join(dict.fromkeys(folders))


@dataclass
class Sandbox:
    root: Path
    tool: str
    binaries: dict[str, Path]
    log: list[dict] = field(default_factory=list)

    @property
    def home(self) -> Path:
        return self.root / "home"

    @property
    def bin(self) -> Path:
        return self.root / "bin"

    def tool_env(self) -> dict[str, str]:
        home = self.home
        return {
            "HOME": str(home),
            "XDG_CONFIG_HOME": str(home / ".config"),
            "XDG_DATA_HOME": str(home / ".local" / "share"),
            "XDG_STATE_HOME": str(home / ".local" / "state"),
            "XDG_CACHE_HOME": str(home / ".cache"),
            "XDG_RUNTIME_DIR": str(home / "run"),
            "DO_NOT_TRACK": "1",
            "QUADRATIQUE_HOME": str(self.root / "qhome"),
            "UV_CACHE_DIR": str(home / ".cache" / "uv"),
        }

    def create(self) -> "Sandbox":
        for sub in ("home/.config", "home/.local/share", "home/.local/state", "home/.cache", "home/run", "qhome", "bin"):
            (self.root / sub).mkdir(parents=True, exist_ok=True)
        (self.home / "run").chmod(0o700)
        # Quadratique refuses a home that a group or other accounts may use.
        (self.root / "qhome").chmod(0o700)
        exports = " ".join(f'{k}="{v}"' for k, v in self.tool_env().items())
        for name, real in self.binaries.items():
            wrapper = self.bin / name
            wrapper.write_text(f'#!/bin/sh\nexec env {exports} "{real}" "$@"\n')
            wrapper.chmod(0o755)
        return self

    def env(self, extra: dict[str, str] | None = None) -> dict[str, str]:
        env = {
            "PATH": f"{self.bin}:{system_path()}",
            "HOME": str(self.home),
            "USER": "maintainer",
            "LANG": "C.UTF-8",
            "TERM": "dumb",
            "GIT_CONFIG_NOSYSTEM": "1",
            "DO_NOT_TRACK": "1",
            # proto's shims (python3, uv) find their tools through PROTO_HOME, not HOME.
            "PROTO_HOME": proto_home(),
        }
        env.update(extra or {})
        return env

    def run(self, args: list[str], cwd: Path, extra: dict[str, str] | None = None, check: bool = True,
            record: bool = False, stdin: str | None = None, timeout: float = 300) -> subprocess.CompletedProcess:
        started = time.time()
        done = processes.run(args, cwd=cwd, env=self.env(extra), stdin=stdin or "", timeout=timeout)
        if record:
            self.log.append({"cmd": args, "cwd": str(cwd), "rc": done.returncode,
                             "seconds": round(time.time() - started, 3)})
        if check and done.returncode != 0:
            raise RuntimeError(f"{args} in {cwd} exited {done.returncode}:\n{done.stdout}\n{done.stderr}")
        return done


def home_files(home: Path) -> list[str]:
    return sorted(str(p.relative_to(home)) for p in home.rglob("*") if p.is_file() or p.is_symlink())


REAL_HOME_PATHS = [".beads", ".config/bd", ".dolt", ".config/lific", ".local/share/lific",
                   ".config/systemd/user/lific.service"]


def real_home_check() -> dict[str, bool]:
    home = Path.home()
    found = {p: (home / p).exists() for p in REAL_HOME_PATHS}
    units = home / ".config" / "systemd" / "user"
    found["any lific systemd unit"] = units.exists() and any("lific" in p.name for p in units.rglob("*"))
    return found
