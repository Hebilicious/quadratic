#!/usr/bin/env bash
set -euo pipefail
repo="$1"
mkdir -p "$repo" && cd "$repo"
git init -q -b main

mkdir -p tally tests
cat > AGENTS.md <<'MD'
# tally

A word counter. Python 3, standard library only. Run the tests with `python3 -m unittest`.

Each agent works in the worktree and branch it was given and commits there. Do not merge into
`main`: the maintainer reviews and merges.

Tasks live in Quadratic. Run `quadratic instructions`.
MD
cat > README.md <<'MD'
# tally

Counts the words of a text file.
MD
cat > tally/__init__.py <<'PY'
from tally.core import count_words

__all__ = ["count_words"]
PY
cat > tally/core.py <<'PY'
def count_words(text):
    """Return a dict that maps each word of the text to the number of times it occurs."""
    raise NotImplementedError
PY
cat > tally/__main__.py <<'PY'
import argparse
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
PY
cat > tests/__init__.py <<'PY'
PY
cat > tests/test_core.py <<'PY'
import unittest

from tally.core import count_words


class CountWords(unittest.TestCase):
    @unittest.skip("count_words is not written yet")
    def test_counts_each_word(self):
        self.assertEqual(count_words("a b a"), {"a": 2, "b": 1})
PY
git add -A && git -c user.name=maintainer -c user.email=maintainer@example.invalid commit -q -m "Start tally"

q() { quadratic "$@"; }
id() { jq -r '.issue.id'; }

q project create --key TAL --name "tally" >/dev/null

t1=$(q create --title "Count the words of a text" --priority 2 \
  --description "count_words(text) returns a dict from word to count. Words are compared in lower case, and punctuation around a word is dropped: 'The cat. The CAT!' gives {'the': 2, 'cat': 2}." \
  --acceptance "The skipped test in tests/test_core.py is enabled and passes. New tests cover upper case, punctuation and an empty text. python3 -m unittest passes." | id)
t2=$(q create --title "Print the N most frequent words" --priority 2 \
  --description "Add --top N to the command line: print only the N most frequent words, most frequent first, ties in alphabetical order." \
  --acceptance "python3 -m tally FILE --top 3 prints three lines. Tests cover the order and the ties. python3 -m unittest passes." | id)
t3=$(q create --title "Ignore stop words" --priority 3 \
  --description "Add --stop-words FILE: a file with one word per line; those words are not counted." \
  --acceptance "Tests cover a stop-words file. python3 -m unittest passes." | id)
t4=$(q create --title "Decide the machine-readable output format" --priority 2 --assignee '{"kind":"human"}' \
  --description "Scripts will read tally's output. Choose the format: JSON object, JSON lines, or TSV. Record the decision here." \
  --acceptance "One format is chosen, with the reason, in a note on this issue." | id)
t5=$(q create --title "Add machine-readable output" --priority 3 \
  --description "Add --format to the command line, with the format that the decision issue chose." \
  --acceptance "Tests cover the chosen format. python3 -m unittest passes." | id)
t6=$(q create --title "Document the command line in the README" --priority 3 --assignee '{"kind":"agent","role":"docs"}' \
  --description "Write a Usage section in README.md that shows every option that exists, each with one example and its real output." \
  --acceptance "Every example in the README was run, and its output is copied as it printed." | id)
t7=$(q create --title "Do not crash on an empty or a binary file" --priority 3 \
  --description "python3 -m tally on an empty file prints nothing and exits 0. On a file that is not UTF-8 text it prints one line to standard error and exits 2, with no traceback." \
  --acceptance "Tests cover both files. python3 -m unittest passes." | id)
t8=$(q create --title "Add --version to the command line" --priority 4 \
  --description "python3 -m tally --version prints 'tally 0.1.0' and exits 0. The version is defined once, in tally/__init__.py." \
  --acceptance "A test covers it. python3 -m unittest passes." | id)

q issue link --issue "$t1" --kind blocks --target "$t2" >/dev/null
q issue link --issue "$t1" --kind blocks --target "$t3" >/dev/null
q issue link --issue "$t4" --kind blocks --target "$t5" >/dev/null
q issue link --issue "$t2" --kind blocks --target "$t6" >/dev/null

echo "tally seeded in $repo: $t1 $t2 $t3 $t4 $t5 $t6 $t7 $t8"
