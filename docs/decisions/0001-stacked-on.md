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
from the branch under review, as long as Quadratique knows how the two branches relate: who may
start the next issue, what happens when the review sends the first issue back, and in what
order the two may merge.

## Decision

Quadratique has a link kind `stacked-on`. `B stacked-on A` means that B's branch starts from A's
branch. A is a *base* of B. The domain rules live in `server/src/domain/stack.rs`; the state of
each base comes from `Board::load` in `server/src/app/board.rs`, which reads the projects and
the stages only of the issues at either end of a `stacked-on` link.

The schema is edited in place, with no migration, until a store is in use: a store made by an
earlier build is recreated.

### Readiness

1. **When B is ready.** While a base is before review, B is not ready. Once every base is in
   review or later, the link no longer keeps B from being ready, and the other readiness rules
   decide. Readiness does not look at the workflow's `claim` status: when that status is at or
   after the `merge` status, B can be ready while `issue.claim` fails with `BaseNotMerged`
   (rule 14).
2. **In review, merged.** A base is merged when its status is the workflow's `merge` status or
   a status after it in the workflow's order, and it is not of the canceled category. Otherwise
   it is in review when its status is the workflow's `review` status or a later one; a workflow
   that orders `merge` before `review` thus makes a base in its `review` status merged. A status
   that the workflow lacks counts as before review. (`Stage::of`.)
3. **A canceled base.** A canceled base never satisfies the link. An unresolved B stays not
   ready, with the reason `stacked` and the base in `blockedBy`, while the base stays
   canceled: until someone removes the link, or the base is reopened and moves on. A resolved
   B never shows `stacked`, whatever its bases.
4. **No wait for resolution.** `stacked-on` does not make B wait for its bases to be resolved.
   Only B's blockers, its unresolved children and the blockers of its ancestors do that; an
   ancestor need not be resolved itself.
5. **More than one base.** An issue may be stacked on more than one issue. Every rule applies
   to each base: B is ready when every base allows it, and B merges after every base.
6. **Direct bases only.** Each rule reads B's direct bases. With C on B on A, C stays
   claimable from B's branch while B is in review, even when A has gone back to work.

### Who may start B: the branch rule

7. **The rule.** While a base is in review, an unclaimed, unresolved B shows `stacked` to a
   caller unless the server verified, as rule 8 says, that the caller's branch starts from the
   base's claim branch. Once the base is merged, the branch rule no longer applies: B then
   starts from the default branch. (`Base::starts`.)
8. **What git verifies.** At the moment of the decision, in the caller's repository and only
   when it is the repository the base's project is bound to, the server reads the local
   branches (`git for-each-ref refs/heads/`) and looks up the caller's branch and the base's
   claim branch by their exact names. The check is positive when both exist and the base
   branch's commit is the caller branch's commit, or an ancestor of it
   (`git merge-base --is-ancestor <commit> <commit>`). It reads branch refs, not the caller's
   checked-out `HEAD` or uncommitted work. A name that is no local branch, such as one with a
   revision suffix, gives no evidence. (`app/ancestry.rs`.)
9. **When git is asked.** Only for `issue.ready`, `issue.list` and `issue.get` without `asOf`,
   and for `issue.claim` after a first decision without git refused the claim with `stacked`
   among its reasons, even when another reason also refuses it. `issue.ready` and
   `issue.list` ask about every unclaimed, unresolved stacked issue on the board whose base in
   review has a claim and is bound to the caller's repository, outside their filters too;
   `issue.get` and `issue.claim` ask only about the issue named. A caller with no repository
   or branch, or a board without a `stacked-on` link, starts no git process for this rule.
   When git fails or times out, the check is negative, so B is not offered.
10. **Other answers.** The answers of other capabilities (`issue.update`, `issue.close`,
    `issue.link` and the others), and `issue.list` and `issue.get` with `asOf`, compute
    readiness without asking git: there, an unclaimed, unresolved issue stacked on a base in
    review shows `stacked` to every caller. A claimed or a resolved issue never gets `stacked`
    from the branch rule.
11. **A claim that exists.** The branch rule decides who may claim B. It never applies to a
    claim that already exists. A link added to a B that someone holds follows the same checks
    as any new link (rule 17 and the cycle check, rule 21); once added, B shows `stacked` while
    a base is before review or canceled, and no reason from the branch rule.
12. **The MCP server.** `quadratique mcp` reads the branch that the session's worktree has at
    each `issue.ready` and `issue.claim`, so a worktree switched to another branch, or
    detached, after the session started counts as it is now. Other calls send the branch read
    when the session started, or when its last claim that named a worktree was taken.

### When a base goes back

13. When a base moves from review back before review, B keeps its claim and its status. An
    unresolved B's `notReady` gains `stacked`, and the base is in `blockedBy`, for as long as
    the base is before review. `issue.get` and `issue.list` show it, and the web app's issue
    page shows it even for an issue in progress, where it shows no other reason.

### Merge order

14. **The guard.** A move fails with `BaseNotMerged`, which names B and every base that is not
    merged, when the status it ends in is the workflow's `merge` status or a later status that
    is not of the canceled category, while a base is not merged. A canceled base is not merged.
    A move into a canceled status, or into any status before the `merge` status, is never
    refused. The guard runs on the outcome of every lifecycle move (`decide` in
    `domain/lifecycle.rs`): `issue.update`, `issue.close` (to the `review` or the `merge`
    status), `issue.claim` and `issue.release` (which a reordered workflow can send past the
    `merge` status), the watcher's landings, and `issue.advance`, which leaves such an issue in
    `left`. (`may_reach`.)
15. **Imports.** A complete `findings.import` finishes an issue whose finding the run lacks
    only when the guard allows a move to the workflow's first completed status. It tries the
    refused issues again as long as a try finished another issue, so a base and an issue
    stacked on it that both vanish in one run both finish, unless another base still keeps the
    stacked one back. An issue that the guard still refuses stays as it is, is in the answer's
    `left`, and the rest of the import goes on. An issue that the import finishes releases the
    held landings stacked on it, in the import's write.
16. **Workflow changes.** `project.update` with a new workflow fails with `MergedBeforeBase`,
    naming the issues, only for an order that the new workflow breaks: a stacked pair with at
    least one end in the edited project, where the issue is at the `merge` status or later
    (not canceled) while the base is not merged under the new workflow, and was not so under
    the current one. A pair already out of order before the edit, such as a base moved back
    after its stacked issue merged, does not block it. In the same write, it releases every
    held landing whose bases all count as merged under the new workflow.
17. **Adding a link.** Adding a link that exists changes nothing and succeeds, whatever the
    state of its issues. A new `stacked-on` link fails with `BaseNotMerged` when the issue is
    at the `merge` status or a later status that is not canceled, and the base is not merged
    (a canceled base is not merged). A canceled issue passes this check whatever its base.

### Landings seen before the bases merge

18. **The landing signals.** The ref watcher reads the default branch and its upstream, and
    moves an issue to the `merge` status on one of these signals:
    - a branch landing: a branch lands on the default branch or its upstream. It names the
      issues whose current claim is on that branch, and the issues whose id the branch's name
      carries. For an issue with a claim, it counts only when the branch is the claim's branch
      and carried commits made since the claim started (the commit the branch pointed to when
      the claim was taken reaches the landed work and differs from it), whatever the issue's
      status, before its `close` too. For an issue without a claim, it counts in any status.
    - a trailer landing: a commit on the default branch or its upstream says `Refs: <issue>`.
      For a claimed issue it counts only in the workflow's `review` status; for an unclaimed
      issue, in any status.

    A resolved issue, or one at the `merge` status, ignores both. A landing that the guard
    refuses is held: the landing is recorded as for any landing, B stays where it is, and B is
    put in the table `held_merges`. (`app/landings.rs`.)
19. **When a hold is assessed again.** After every move of an issue, after each issue an
    import finishes, after a `stacked-on` link is removed and after a workflow change,
    Quadratique decides again for each held issue concerned. B moves to the `merge` status once
    the guard allows it, and the hold ends; B's move is part of the same store commit as the
    move that released it, with that commit's author and operation. When B is already resolved
    or at the `merge` status, the hold is discarded and B does not move. (`release_held` in
    `app/lifecycle.rs`.)
20. **When a hold ends.** A hold belongs to the landing that made it, and ends with the work
    of that landing: on any move that ends or starts B's claim (a release, a new claim, a move
    to a canceled, unstarted, backlog or completed status, or into the `merge` status, such as
    a `close` from the default branch), when an import finishes B, and when an assessment finds
    B resolved or at the `merge` status. A move that keeps the claim, such as from review back
    to in progress, keeps the hold. (`apply` in `app/lifecycle.rs`.)

### Cycles

21. The cycle check treats `stacked-on` and `blocks` as one graph, together with children and
    ancestors. A new link of either kind that would close a loop through any mix of them fails
    with `Cycle`.

### States and events

The state of B for one base A. Each row is a stage of A; each column is an event. The cell
gives what happens to B. A claim cell assumes that B passes the other readiness rules and that
the workflow's `claim` status is before its `merge` status; otherwise the claim fails for that
reason. "Merge or later" means the workflow's `merge` status or a later status that is not
canceled.

| A's stage | B claim, verified to start from A's branch (rule 8) | B claim, not so verified | A move of B to merge or later | Branch landing of B | Trailer landing of B (B in review, or unclaimed) |
|---|---|---|---|---|---|
| Before review | `NotReady` (`stacked`, A) | `NotReady` (`stacked`, A) | `BaseNotMerged` | held | held |
| In review | claimed | `NotReady` (`stacked`, A) | `BaseNotMerged` | held | held |
| Merged | claimed | claimed | moves | moves | moves |
| Canceled | `NotReady` (`stacked`, A) | `NotReady` (`stacked`, A) | `BaseNotMerged` | held | held |

What each event does to B:

| Event | Effect on B |
|---|---|
| A moves to review | An unclaimed B becomes ready, if the other rules allow it, for callers verified to start from A's branch. A claimed B loses `stacked`. |
| A goes back before review | An unclaimed B is not ready. A claimed B keeps its claim and status, and shows `stacked` with A. |
| A is canceled | An unresolved B is not ready, with `stacked` and A, while A stays canceled. |
| A reaches the `merge` status, by any move, an import or a workflow change | The branch rule no longer applies to B. A held B moves to the `merge` status if no other base holds it. |
| The link is removed | B is free of A. A held B moves to the `merge` status if no other base holds it. |
| B is canceled, released, claimed anew, moved out of the started category, moved into the `merge` status, or finished by an import | B's hold, if any, ends. |
| A new link is added while B is at merge or later and A is not merged | Refused: `BaseNotMerged`. |
| The link is added again | Nothing changes. |
| A workflow change would newly put B at merge or later ahead of A | Refused: `MergedBeforeBase`. |

The tests take their names from these rules: `server/tests/domain/stack.rs`,
`server/tests/server/stacked.rs`, the cycle property in `server/tests/domain/waits.rs`, and the
session test in `cli/src/mcp.rs`.

## Known limits

- A finding that reappears is reopened by `findings.import` without the merge guard and
  without assessing holds again. This matters only under a workflow that orders its first
  unstarted status after its `merge` status.
- A landing that the watcher attributes through a claim can be recorded just after that claim
  was released, and then holds the issue again.
- The branch rule takes the repository that the base's project is bound to as the place of the
  base's branch. A base claimed without a named worktree from another repository is not told
  apart.
- For a claim of an issue with several bases, the git checks are not taken at one commit of
  the caller's branch, and are not tied to the base claims they were made for.
- A claim that names a worktree and waits its turn in the MCP server sends the branch read
  before the wait.
- `issue.ready` and `issue.list` ask git about every candidate base on the board in the
  caller's repository, one process each, with no deadline for the whole request.
- Several tests cover behaviour that existed before the fix they were written for: the
  multi-base hold, `issue.advance` and `left`, the link to a claimed issue, and the browser
  assertion. The cancel-then-claim test does not show which of the two moves ends the hold, and
  the git-count test counts calls through the server's own counter.

## Alternatives considered

- **Use `blocks`.** It makes B wait until A is resolved, after the merge, which is the wait
  that this link removes.
- **Hold B's landing by ignoring it, as for a trailer landing before `close`.** B would then
  stay in review after its bases merge, and someone would have to move it by hand. A held
  landing is known to be B's, so Quadratique moves B itself.
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
