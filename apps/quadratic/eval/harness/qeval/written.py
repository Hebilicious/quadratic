from pathlib import Path

from qeval import tally

SKILL_ROOTS = (".agents/skills/", ".claude/skills/", ".codex/skills/")


def skill_folder(path: str) -> str | None:
    for root in SKILL_ROOTS:
        name, slash, _ = path.removeprefix(root).partition("/")
        if path.startswith(root) and slash:
            return f"{root}{name}/"
    return None


def paths(entry: str) -> list[str]:
    """A rename or copy stored by an older run names its source and its destination."""
    status, rest = entry.split("\t", 1)
    return rest.split("\t") if status[0] in "RC" else [rest]


def counted_files(name_status: list[str]) -> int:
    """The files of one skill folder count as one file. A folder is a
    skill when its SKILL.md is among the files; any other file counts on its own."""
    written = [path for entry in name_status for path in paths(entry)]
    skills = {folder for path in written if (folder := skill_folder(path)) and path == f"{folder}SKILL.md"}
    return len({folder if (folder := skill_folder(path)) in skills else path for path in written})


def name_status(repo: Path, base: str, head: str, env: dict[str, str]) -> list[str]:
    """`-z` keeps each path as written (git quotes unusual ones otherwise); `--no-renames` gives two fields per entry."""
    fields = tally.git(repo, "diff", "-z", "--no-renames", "--name-status", base, head, env=env).split("\0")
    return [f"{status}\t{path}" for status, path in zip(fields[0::2], fields[1::2])]
