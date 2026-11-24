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
branch. A is a *base* of B. The domain rules live in `server/src/domain/stack.rs`, one function
for each rule below; the state of each base comes from `Board::load` in
`server/src/app/board.rs`.

### Rules

1. **When B is ready.** B is ready, and can be claimed, once every base is in review or later,
   and not before. All other readiness rules still apply.
2. **In review or later.** A base is in review when its status is the workflow's `review`
   status or a status after it in the workflow's order, and it is not of the canceled category.
   A base at the workflow's `merge` status or after it, and not canceled, is merged. A status
   that the workflow lacks counts as before review. (`Stage::of`.)
3. **A canceled base.** A canceled base never satisfies the link. B stays not ready, with the
   reason `stacked` and the base in `blockedBy`, until someone removes the link.
4. **Who may start B.** While a base is in review, B is ready only for a caller whose branch
   is the base's claim branch, or contains that branch's tip. The same rule decides
   `issue.claim`, because a claim takes only a ready issue; for a claim that names a
   `worktree`, the caller's branch is the branch of that worktree. (`Base::starts`.)
5. **Once a base is merged.** A merged base no longer limits who may start B: B then starts from
   the default branch.
6. **An unknown branch.** The rule never offers B to a caller it would exclude. When the
   server cannot read which branches the caller's branch contains (no repository, a detached
   `HEAD`, git fails), or the base in review has no claim, or its branch was deleted, B is not
   ready for that caller until the base is merged.
7. **B merges after its bases.** A call that would move B to the workflow's `merge` status, or
   to a later status that is not canceled, fails with `BaseNotMerged`, which names B and every
   base that is not merged. This covers `issue.update` and `issue.close`. `issue.advance` leaves
   such an issue in `left`. Moving B to a canceled status, or to any status before the merge,
   stays allowed. (`may_reach`.)
8. **A landing of B before its bases.** When the ref watcher sees B land on the default branch
   while a base is not merged, B stays where it is. The landing is recorded as for any landing,
   and B is put in the table `held_merges`. As soon as an issue reaches the `merge` status, or
   a `stacked-on` link is removed, Quadratic decides again for each held issue stacked on it:
   B moves to the `merge` status once every base is merged (`release_held` in
   `server/src/app/lifecycle.rs`). B's move is part of the same store commit as the move that
   released it, with that commit's author and operation. This differs from a landing seen
   before an issue's `close`, which is recorded and then ignored: here the landing is sure to
   be B's, and only the order of the merges is wrong.
9. **When a base goes back.** When a base moves from review back before review, B keeps its
   claim and its status. B's `notReady` gains `stacked`, and the base is in `blockedBy`, for as
   long as the base is before review. `issue.get` and `issue.list` show it, and
   the web app's issue page shows it even for an issue in progress, where it shows no other
   reason. A claimed B never gets `stacked` from the branch rule: its holder already started.
   A resolved B never gets `stacked`.
10. **No wait for resolution.** `stacked-on` does not make B wait for its bases to be resolved.
    Only `blocks`, children and ancestors do that.
11. **Cycles.** The cycle check treats `stacked-on` and `blocks` as one graph, together with
    children and ancestors. A link of either kind that would close a loop through any mix of
    them fails with `Cycle`.
12. **More than one base.** An issue may be stacked on more than one issue. Every rule above
    applies to each base: B is ready when every base allows it, and B merges after every base.
    This needs no extra design, so no limit applies.

### How a caller's branches are read

At each request that carries a repository and a branch, the server runs
`git for-each-ref --merged=refs/heads/<branch> refs/heads/` in that repository and keeps the
names of the local branches that the caller's branch contains (`with_branches` in
`server/src/app/caller.rs`). This costs one git process for each such request. Readiness
stays a pure function of the store and the caller.

### States and events

The state of B for one base A. Each row is a stage of A; each column is an event. The cell
gives what happens to B.

| A's stage | B claim, from A's branch or one on top | B claim, from another branch | Call moves B to `merge` or later | Watcher sees B land |
|---|---|---|---|---|
| Before review | `NotReady` (`stacked`, A) | `NotReady` (`stacked`, A) | `BaseNotMerged` | held |
| In review | claimed | `NotReady` (`stacked`, A) | `BaseNotMerged` | held |
| Merged | claimed | claimed | moves | moves |
| Canceled | `NotReady` (`stacked`, A) | `NotReady` (`stacked`, A) | `BaseNotMerged` | held |

What each move of A, or of the link, does to B:

| Event | Effect on B |
|---|---|
| A moves to review | Unclaimed B becomes ready for callers on A's branch or on top of it. A claimed B loses `stacked`. |
| A goes back before review | Unclaimed B is not ready. Claimed B keeps its claim and status, and shows `stacked` with A. |
| A is canceled | B is not ready, with `stacked` and A, until the link is removed. |
| A reaches the `merge` status | B is ready for any caller. A held B moves to the `merge` status if no other base holds it. |
| The link is removed | B is free of A. A held B moves to the `merge` status if no other base holds it. |

The tests take their names from these rules: `server/tests/domain/stack.rs`,
`server/tests/server/stacked.rs`, and the cycle property in `server/tests/domain/waits.rs`.

## Alternatives considered

- **Use `blocks`.** It makes B wait until A is resolved, after the merge, which is the wait
  that this link removes.
- **Hold B's landing by ignoring it, as for a landing before `close`.** B would then stay in
  review after its bases merge, and someone would have to move it by hand. A held landing is
  known to be B's, so Quadratic moves B itself.
- **At most one base for each issue.** The rules generalise to several bases without new
  states, so the limit would only refuse a valid plan.
- **Offer B to every caller while a base is in review.** An agent on an unrelated branch would
  then start B without the base's work. The branch rule costs one git process for each request
  and keeps B for the callers that have the base's commits.
