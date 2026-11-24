# 0001. The link kind `stacked-on`

Date: 2026-11-24

## Context

An agent finishes an issue, closes it, and asks a human for a review. The issue stays in
review until the human accepts it and its branch lands on the default branch. Until now the
agent had two choices while it waited: stop, or start unrelated work. Work that builds on the
branch under review could not be tracked: a `blocks` link keeps the next issue from being ready
until the first one is resolved, which is after the merge, and no link said that the next
branch starts from the first one.

Waiting for a review is not a hard blocker. The agent can keep going on a branch that starts
from the branch under review, as long as Quadratic knows how the two branches relate: who may
start the next issue, what happens when the review sends the first issue back, and in what
order the two may merge.

## Decision

Quadratic has a link kind `stacked-on`. `B stacked-on A` means that B's branch starts from A's
branch. A is a *base* of B. The domain rules live in `server/src/domain/stack.rs`; the state of
each base comes from `Board::load` in `server/src/app/board.rs`, which reads the projects and
the stages only of the issues at either end of a `stacked-on` link.

The schema is edited in place, with no migration, until a store is in use: a store made by an
earlier build is recreated.

### Readiness

1. **When B is ready.** B is ready, and can be claimed, once every base is in review or later,
   and not before. All other readiness rules still apply.
2. **In review or later.** A base is in review when its status is the workflow's `review`
   status or a status after it in the workflow's order, and it is not of the canceled category.
   A base at the workflow's `merge` status or after it, and not canceled, is merged. A status
   that the workflow lacks counts as before review. (`Stage::of`.)
3. **A canceled base.** A canceled base never satisfies the link. B stays not ready, with the
   reason `stacked` and the base in `blockedBy`, while the base stays canceled: until someone
   removes the link, or the base is reopened and moves on.
4. **No wait for resolution.** `stacked-on` does not make B wait for its bases to be resolved.
   Only `blocks`, children and ancestors do that.
5. **More than one base.** An issue may be stacked on more than one issue. Every rule applies
   to each base: B is ready when every base allows it, and B merges after every base.
6. **Direct bases only.** Each rule reads B's direct bases. With C on B on A, C stays
   claimable from B's branch while B is in review, even when A has gone back to work.

### Who may start B: the branch rule

7. **The rule.** While a base is in review, B is ready only for a caller that works in the
   repository the base's project is bound to, on a branch that is the base's claim branch or
   contains that branch's tip. A branch of the same name in another repository never counts.
   Once the base is merged, B is ready for any caller: B then starts from the default branch.
   (`Base::starts`.)
8. **How git is asked.** The server runs
   `git merge-base --is-ancestor refs/heads/<base branch> refs/heads/<caller branch>` in the
   caller's repository, once for each base branch, and only when a decision needs it: for
   `issue.ready`, `issue.list` and `issue.get` (without `asOf`), and for `issue.claim` after a
   first decision without git refused the claim for `stacked`. It asks only when the caller's
   repository is the base's project's repository, the base is in review with a claim, and the
   issue stacked on it is unclaimed and unresolved. A board without a `stacked-on` link starts
   no git process for this rule. (`app/ancestry.rs`, `Board::branches_to_check`.)
9. **When git cannot tell.** The rule never offers B to a caller it would exclude. When the
   caller has no repository or branch, when git fails or times out, when the base in review has
   no claim or its project no repository, or when its branch was deleted, B is not ready for
   that caller until the base is merged.
10. **Other answers.** The answers of other capabilities (`issue.update`, `issue.close`,
    `issue.link` and the others) compute readiness without asking git: there, an issue stacked
    on a base in review shows `stacked` to every caller. `issue.ready`, `issue.list`,
    `issue.get` and `issue.claim` decide.
11. **A claim that exists.** The branch rule decides who may claim B. It never applies to a
    claim that already exists, so a link added to a B that someone holds is allowed: B then
    shows `stacked` while a base is before review or canceled, and no reason from the branch
    rule.
12. **The MCP server.** `quadratic mcp` reads the branch that the session's worktree has at
    each `issue.ready` and `issue.claim`, so a worktree switched to another branch, or
    detached, after the session started counts as it is now. Other calls send the branch read
    when the session started, or when its last claim that named a worktree was taken.

### When a base goes back

13. When a base moves from review back before review, B keeps its claim and its status. B's
    `notReady` gains `stacked`, and the base is in `blockedBy`, for as long as the base is
    before review. `issue.get` and `issue.list` show it, and the web app's issue page shows it
    even for an issue in progress, where it shows no other reason. A resolved B never gets
    `stacked`.

### Merge order

14. **The guard.** No move puts B at the workflow's `merge` status, or at a later status that
    is not canceled, while a base is not merged: it fails with `BaseNotMerged`, which names B
    and every base that is not merged. The guard runs on the outcome of every lifecycle move
    (`decide` in `domain/lifecycle.rs`): `issue.update`, `issue.close`, `issue.claim` and
    `issue.release` (which a reordered workflow can send past the `merge` status), the
    watcher's landings and `issue.advance`, which leaves such an issue in `left`. Moving B to a
    canceled status, or to any status before the `merge` status, stays allowed. (`may_reach`.)
15. **Imports.** A complete `findings.import` finishes an issue whose finding the run lacks
    only when the guard allows it. It tries the refused issues again as long as a try finished
    another issue, so a base and an issue stacked on it that both vanish in one run both
    finish. An issue that the guard
    still refuses stays as it is, is in the answer's `left`, and the rest of the import goes
    on. An issue that the import finishes releases the held landings stacked on it, in the
    import's write.
16. **Workflow changes.** `project.update` with a new workflow fails with `MergedBeforeBase`,
    naming the issues, when under it an issue would be at the `merge` status or later while
    one of its bases is not merged. In the same write, it releases every held landing whose
    bases all count as merged under the new workflow.
17. **Adding a link late.** A `stacked-on` link from an issue at the `merge` status or later to
    a base that is not merged fails with `BaseNotMerged`.

### Landings seen before the bases merge

18. **The two landing signals.** The ref watcher moves an issue to the `merge` status on one of
    two signals. A branch landing: the branch of the issue's current claim lands on the default
    branch with commits made since the claim; it counts whatever the claimed issue's status,
    before its `close` too. A trailer landing: a commit on the default branch says
    `Refs: <issue>`; for a claimed issue it counts only in the workflow's `review` status, and
    for an unclaimed issue in any status. A resolved issue, or one at the `merge` status,
    ignores both. A landing that the guard refuses is held: the
    landing is recorded as for any landing, B stays where it is, and B is put in the table
    `held_merges`. (`app/landings.rs`.)
19. **When a hold moves B.** After every move of an issue, after each issue an import
    finishes, after a `stacked-on` link is removed and after a workflow change, Quadratic
    decides again for each held issue concerned: B moves to the `merge` status once the guard
    allows it, and the hold ends. B's move is part of the same store commit as the move that
    released it, with that commit's author and operation. (`release_held` in
    `app/lifecycle.rs`.)
20. **When a hold ends.** A hold belongs to the landing that made it, and ends with the work
    of that landing: on any move that ends or starts B's claim (a release, a new claim, a move
    to a canceled, unstarted, backlog or completed status, or into the `merge` status), and when
    an import finishes B. A move that keeps the claim, such as from review back to in progress,
    keeps the hold. (`apply` in `app/lifecycle.rs`.)

### Cycles

21. The cycle check treats `stacked-on` and `blocks` as one graph, together with children and
    ancestors. A link of either kind that would close a loop through any mix of them fails with
    `Cycle`.

### States and events

The state of B for one base A. Each row is a stage of A; each column is an event. The cell
gives what happens to B.

| A's stage | B claim, from A's branch or one on top, in A's repository | B claim, from anywhere else | Any move of B to `merge` or later | Branch landing of B's current claim | Trailer landing of B (B in review, or unclaimed) |
|---|---|---|---|---|---|
| Before review | `NotReady` (`stacked`, A) | `NotReady` (`stacked`, A) | `BaseNotMerged` | held | held |
| In review | claimed | `NotReady` (`stacked`, A) | `BaseNotMerged` | held | held |
| Merged | claimed | claimed | moves | moves | moves |
| Canceled | `NotReady` (`stacked`, A) | `NotReady` (`stacked`, A) | `BaseNotMerged` | held | held |

What each event does to B:

| Event | Effect on B |
|---|---|
| A moves to review | Unclaimed B becomes ready for callers on A's branch or on top of it, in A's repository. A claimed B loses `stacked`. |
| A goes back before review | Unclaimed B is not ready. Claimed B keeps its claim and status, and shows `stacked` with A. |
| A is canceled | B is not ready, with `stacked` and A, while A stays canceled. |
| A reaches the `merge` status, by any move, an import or a workflow change | B is ready for any caller. A held B moves to the `merge` status if no other base holds it. |
| The link is removed | B is free of A. A held B moves to the `merge` status if no other base holds it. |
| B is canceled, released, claimed anew, or moved out of the started category | B's hold, if any, ends. |
| The link is added while B is at the `merge` status or later and A is not merged | Refused: `BaseNotMerged`. |
| A workflow change would put B at the `merge` status or later ahead of A | Refused: `MergedBeforeBase`. |

The tests take their names from these rules: `server/tests/domain/stack.rs`,
`server/tests/server/stacked.rs`, the cycle property in `server/tests/domain/waits.rs`, and the
session test in `cli/src/mcp.rs`.

## Alternatives considered

- **Use `blocks`.** It makes B wait until A is resolved, after the merge, which is the wait
  that this link removes.
- **Hold B's landing by ignoring it, as for a trailer landing before `close`.** B would then
  stay in review after its bases merge, and someone would have to move it by hand. A held
  landing is known to be B's, so Quadratic moves B itself.
- **At most one base for each issue.** The rules generalise to several bases without new
  states, so the limit would only refuse a valid plan.
- **Offer B to every caller while a base is in review.** An agent on an unrelated branch would
  then start B without the base's work.
- **Read the caller's branches at every request.** It made every request from a repository
  start a git process, on a path the caller names, before the capability was known. Asking git
  only for a decision about a base in review, in the project's repository, costs nothing on a
  board without the link.
- **Rules over the whole stack.** Making C wait for every base below B would need the rules to
  walk the stack; the direct-base rule is enough for an agent that brings its base's commits
  into its branch when the base goes back.
