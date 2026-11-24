from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path

from qeval import processes
from qeval.sandbox import MAINTAINER

FILES = {
    "AGENTS.md": """# tally

A word counter. Python 3, standard library only. Run the tests with `python3 -m unittest`.

Each agent works in the worktree and branch it was given and commits there. Do not merge into
`main`: the maintainer reviews and merges.
""",
    "README.md": "# tally\n\nCounts the words of a text file.\n",
    "tally/__init__.py": 'from tally.core import count_words\n\n__all__ = ["count_words"]\n',
    "tally/core.py": '''def count_words(text):
    """Return a dict that maps each word of the text to the number of times it occurs."""
    raise NotImplementedError
''',
    "tally/__main__.py": '''import argparse
import sys

from tally.core import count_words


def main(argv=None):
    parser = argparse.ArgumentParser(prog="tally", description="Count the words of a text file.")
    parser.add_argument("file")
    args = parser.parse_args(argv)
    with open(args.file, encoding="utf-8") as handle:
        counts = count_words(handle.read())
    for word, count in counts.items():
        print(f"{word} {count}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
''',
    "tests/__init__.py": "",
    "tests/test_core.py": '''import unittest

from tally.core import count_words


class CountWords(unittest.TestCase):
    @unittest.skip("count_words is not written yet")
    def test_counts_each_word(self):
        self.assertEqual(count_words("a b a"), {"a": 2, "b": 1})
''',
}


@dataclass(frozen=True)
class Human:
    pass


@dataclass(frozen=True)
class Role:
    name: str


Assignee = Human | Role


@dataclass
class Spec:
    key: str
    title: str
    priority: int
    description: str
    acceptance: str = ""
    assignee: Assignee | None = None
    blocks: list[str] = field(default_factory=list)
    paths: tuple[str, ...] = ()
    enables: tuple[str, ...] = ()
    tested: bool = False


ISSUES = [
    Spec("t1", "Count the words of a text", 2,
         "count_words(text) returns a dict from word to count. Words are compared in lower case, and punctuation around a word is dropped: 'The cat. The CAT!' gives {'the': 2, 'cat': 2}.",
         "The skipped test in tests/test_core.py is enabled and passes. New tests cover upper case, punctuation and an empty text. python3 -m unittest passes.",
         blocks=["t2", "t3"], paths=("tally/core.py",), enables=("test_counts_each_word",), tested=True),
    Spec("t2", "Print the N most frequent words", 2,
         "Add --top N to the command line: print only the N most frequent words, most frequent first, ties in alphabetical order.",
         "python3 -m tally FILE --top 3 prints three lines. Tests cover the order and the ties. python3 -m unittest passes.",
         blocks=["t6"], paths=("tally/__main__.py", "tally/core.py"), tested=True),
    Spec("t3", "Ignore stop words", 3,
         "Add --stop-words FILE: a file with one word per line; those words are not counted.",
         "Tests cover a stop-words file. python3 -m unittest passes.", paths=("tally/__main__.py", "tally/core.py"), tested=True),
    Spec("t4", "Decide the machine-readable output format", 2,
         "Scripts will read tally's output. Choose the format: JSON object, JSON lines, or TSV. Record the decision here.",
         "One format is chosen, with the reason, in a note on this issue.", assignee=Human(), blocks=["t5"]),
    Spec("t5", "Add machine-readable output", 3,
         "Add --format to the command line, with the format that the decision issue chose.",
         "Tests cover the chosen format. python3 -m unittest passes.", paths=("tally/__main__.py", "tally/core.py"), tested=True),
    Spec("t6", "Document the command line in the README", 3,
         "Write a Usage section in README.md that shows every option that exists, each with one example and its real output.",
         "Every example in the README was run, and its output is copied as it printed.",
         assignee=Role("docs"), paths=("README.md",)),
    Spec("t7", "Do not crash on an empty or a binary file", 3,
         "python3 -m tally on an empty file prints nothing and exits 0. On a file that is not UTF-8 text it prints one line to standard error and exits 2, with no traceback.",
         "Tests cover both files. python3 -m unittest passes.", paths=("tally/__main__.py",), tested=True),
    Spec("t8", "Add --version to the command line", 4,
         "python3 -m tally --version prints 'tally 0.1.0' and exits 0. The version is defined once, in tally/__init__.py.",
         "A test covers it. python3 -m unittest passes.", paths=("tally/__init__.py", "tally/__main__.py"), tested=True),
]

def git(repo: Path, *args: str, env: dict[str, str], hooks: bool = False) -> str:
    hook_setting = [] if hooks else ["-c", "core.hooksPath=/dev/null"]
    done = processes.run(["git", *hook_setting, *args], cwd=repo, env=env, timeout=300)
    if done.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} in {repo} exited {done.returncode}: {done.stderr}")
    return done.stdout


def create_repo(repo: Path, env: dict) -> None:
    repo.mkdir(parents=True)
    git(repo, "init", "-q", "-b", "main", env=env)
    git(repo, "config", "user.name", MAINTAINER[0], env=env)
    git(repo, "config", "user.email", MAINTAINER[1], env=env)
    for name, text in FILES.items():
        path = repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    git(repo, "add", "-A", env=env)
    git(repo, "commit", "-q", "-m", "Start tally", env=env, hooks=True)


def commit_all(repo: Path, env: dict, message: str) -> bool:
    git(repo, "add", "-A", env=env, hooks=True)
    if not git(repo, "status", "--porcelain", env=env).strip():
        return False
    git(repo, "commit", "-q", "-m", message, env=env, hooks=True)
    return True
