#!/usr/bin/env bash
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$1"
mkdir -p "$repo" && cd "$repo"
git init -q -b main

mkdir -p tally tests prd .agents/skills/quadratique .claude
cp "$here"/../skill/quadratique/*.md .agents/skills/quadratique/
ln -s ../.agents/skills .claude/skills
ln -s AGENTS.md CLAUDE.md
touch prd/.gitkeep tests/__init__.py
cat > AGENTS.md <<'FILE'
# tally

A word counter. Python 3, standard library only. Run the tests with `python3 -m unittest`.

## Tasks

- We use Quadratique for issues: every task is an issue there. Use the `quadratique` skill
  (`.agents/skills/quadratique/SKILL.md`) for anything about tasks.
- Product plans are PRDs in `prd/`.

## Branches and review

- `main` changes only through a merge. Each task has its own branch and worktree:
  `git worktree add .worktrees/<branch> -b <branch> main`.
- Before a branch is merged back into `main`, it needs a review by the maintainer. When the work
  is finished, ask for the review in the session: the issue, the branch, what changed, and the test
  results. Then wait for the answer.
- After the maintainer accepts: from the main checkout, `git merge --no-ff <branch>`, run the tests
  on `main`, and remove the worktree.
FILE
cat > README.md <<'FILE'
# tally

Counts the words of a text file.

```sh
python3 -m tally FILE
```
FILE
cat > .gitignore <<'FILE'
__pycache__/
.worktrees/
FILE
cat > tally/__init__.py <<'FILE'
from tally.core import count_words

__all__ = ["count_words"]
FILE
cat > tally/core.py <<'FILE'
import string


def count_words(text):
    """Return a dict that maps each word of the text to the number of times it occurs."""
    counts = {}
    for token in text.split():
        word = token.strip(string.punctuation).lower()
        if word:
            counts[word] = counts.get(word, 0) + 1
    return counts
FILE
cat > tally/__main__.py <<'FILE'
import argparse
import sys

from tally.core import count_words


def main(argv=None):
    parser = argparse.ArgumentParser(prog="tally", description="Count the words of a text file.")
    parser.add_argument("file")
    args = parser.parse_args(argv)
    with open(args.file, encoding="utf-8") as handle:
        counts = count_words(handle.read())
    for word, count in sorted(counts.items(), key=lambda item: (-item[1], item[0])):
        print(f"{word} {count}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
FILE
cat > tests/test_core.py <<'FILE'
import unittest

from tally.core import count_words


class CountWords(unittest.TestCase):
    def test_counts_each_word(self):
        self.assertEqual(count_words("a b a"), {"a": 2, "b": 1})

    def test_ignores_case_and_punctuation(self):
        self.assertEqual(count_words("The cat. The CAT!"), {"the": 2, "cat": 2})
FILE
git add -A && git -c user.name=maintainer -c user.email=maintainer@example.invalid commit -q -m "Start tally"

quadratique project create --key TAL --name "tally" >/dev/null
echo "tally seeded in $repo, project TAL, no issues"
