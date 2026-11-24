#!/usr/bin/env bash
set -euo pipefail
repo="$1"
mkdir -p "$repo" && cd "$repo"
git init -q -b main

cat > AGENTS.md <<'MD'
# pomo

A work timer in POSIX shell. Run the tests with `sh test.sh`.

Each agent works in the worktree and branch it was given and commits there. Do not merge into
`main`: the maintainer reviews and merges.

Tasks live in Quadratic. Run `quadratic instructions`.
MD
cat > pomo.sh <<'SH'
#!/bin/sh
# pomo MINUTES: prints the time left once per second, then "done".
minutes="$1"
seconds=$((minutes * 60))
while [ "$seconds" -gt 0 ]; do
  printf '%s\n' "$seconds"
  sleep 1
  seconds=$((seconds - 1))
done
echo done
SH
cat > test.sh <<'SH'
#!/bin/sh
set -e
out=$(sh pomo.sh 0)
[ "$out" = "done" ] || { echo "pomo 0 should print done, got: $out"; exit 1; }
echo ok
SH
git add -A && git -c user.name=maintainer -c user.email=maintainer@example.invalid commit -q -m "Start pomo"

q() { quadratic "$@"; }
id() { jq -r '.issue.id'; }

q project create --key POM --name "pomo" >/dev/null
p1=$(q create --title "Refuse a duration that is not a whole number" --priority 2 \
  --description "pomo.sh abc, pomo.sh -1 and pomo.sh with no argument print a usage line to standard error and exit 2." \
  --acceptance "test.sh covers the three cases and passes." | id)
p2=$(q create --title "Print the time left as MM:SS" --priority 3 \
  --description "Print 01:30 where pomo prints 90. Add a variable POMO_TICK (seconds per tick, default 1) so that a test can run without waiting." \
  --acceptance "test.sh checks the first lines of a one-minute timer with POMO_TICK=0 and passes." | id)
echo "pomo seeded in $repo: $p1 $p2"
