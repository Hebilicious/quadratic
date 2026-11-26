# 0002. Claims of agents that ended

Date: 2026-11-25

## Context

A lead agent starts other agents. One of them claims an issue, closes it into review and ends.
The lead, or a reviewer, then needs to add a note to that issue, or to free it when the work
will not be merged (an audit, for example).

The rules are in `Rules` in `server/src/domain/lifecycle.rs`. Who holds a claim is
`Caller::holds` in `server/src/domain/issue.rs` (rule 2).

The server knows of an agent only what the agent's actor says (`server/src/domain/actor.rs`):

- `session` comes from the harness: `CLAUDE_CODE_SESSION_ID` for Claude Code,
  `CODEX_THREAD_ID` for Codex.
- `host` and `hostThread` come from the host: `T3CODE_THREAD_ID` for T3 Code.
- `parent` comes only from `QUADRATIQUE_ACTOR_FIELDS`, the supported override of actor fields.
  No harness or host sets it.

An agent that a T3 Code thread delegates to has its own session id. Its `hostThread` is an
opaque id of the delegated task, which does not contain the lead's thread or session. The
server can receive a declared `parent` through the override, but it has no parent link that
it can trust: the agent declares the value itself. It also has no signal that a holder is
still alive: a claim records no liveness.

## Decision

1. **Notes.** Any actor may add a note to any issue, claimed or not, in any status:
   `issue.update` with `notes` and the issue's current `revision`. A review note reaches an
   issue in review this way, from a human or from any agent. Fields without a status
   (`description`, `acceptance`, `priority`, `notes`) are open to every actor; an agent
   cannot change the assignee of an issue assigned to the human (`HumanOnly`, except as
   rule 4 says).
2. **Manual moves by agents.** This rule is about the moves an actor asks for: a change of
   status (`issue.update` with `status`), `issue.release` and `issue.close`. Of the actors
   that do not hold a claim, only the human may make these moves on the claimed issue, except
   the release of rule 3. The rules refuse in this order, and the first refusal that applies
   is the answer. Rule 4 says what the human's words change at the place of `HumanOnly`:
   - `issue.update` with `status`: `Stale` for an old `revision`; `UnknownStatus`; then
     `HumanOnly` for an agent on an issue assigned to the human, whether or not it holds the
     claim; then `Held`, which names the holder, for an agent that does not hold it.
   - `issue.release`: an unclaimed issue does not change; then `HumanOnly` as above; then,
     for an agent that does not hold the claim, the release of rule 3 on an issue in review,
     and `Held` on an issue before review.
   - `issue.close`: `Resolved` for a canceled issue; success with no change for a completed
     issue or one in the workflow's `merge` status; then `HumanOnly` as above; then `Held`
     for an agent that does not hold the claim, or `NotClaimed` when nobody holds it.
   - Then, for every caller, the human too: a move that would end at the workflow's `merge`
     status or later fails with `BaseNotMerged` while an issue it is stacked on is not
     merged (decision 0001, rule 14). The human closing a stacked issue before its base
     merges gets `BaseNotMerged`, because the human's close moves the issue to the `merge`
     status.

   The holder is the same actor name in the same worktree, or, from any worktree, an agent
   with the holder's `harness` and `session`, both given and not blank, when the holder is an
   agent too: the match that `issue.list --claim mine` uses (`Actor::agent_session`). A caller
   with no session, with a blank one, with a session but no harness, with another session, or
   with the holder's session id under another harness is not the holder outside the claim's
   worktree. A blank `harness` or `session` (empty after trimming) counts as absent wherever
   an actor comes in (`Actor::detect`, the actor header); other values compare exactly.

   A human is never the holder's session, whatever fields it declares. The first version of
   this rule ignored the actor's kind, to stay the exact match of `--claim mine` at the time.
   A review showed what that opened: a human that declared an agent's harness and session took
   the agent's `close` path, moved the issue to review with the claim kept, and skipped the
   `BaseNotMerged` check of the human's close. The match is now between agents, in the rule
   and in `--claim mine`: a human's `--claim mine` lists the claims of its own name in this
   worktree, as it did for a human without declared session fields. `HumanOnly` and
   `BaseNotMerged` keep their place in the order above for the holder's session too.

   A move by the holder's session from another worktree changes nothing in the claim: it
   keeps its holder, its `branch` and its `worktree`. The merge watcher matches a landing to
   the claim's `branch`, and the trailer `Refs:` lands an issue in review whatever the
   branch, so a `close` from elsewhere does not change what counts as the issue's merge.
   `issue.claim` by the holder's session from another worktree is the holder's repeated call:
   it changes nothing.

   Every actor field is self-declared. A caller that declares another agent's harness and
   session, through `QUADRATIQUE_ACTOR_FIELDS` or its environment, is that agent's holder
   from any worktree. The server cannot detect this: it has no secret per session. Before
   this rule, the same caller could declare the holder's name and send the holder's worktree
   path, with the same effect. The rule protects against mistakes of honest agents (another
   session, a missing session, a session id that two harnesses share), not against an agent
   that lies.
3. **A claim whose holder ended.** Any agent may release another agent's claim on an issue
   in review: the workflow's `review` status, or a later status before `merge`. The owner
   decided this on 2026-11-25. The issue keeps its status, its evidence and its assignee, and
   has no claim. It gains a note, written by the releasing agent, that names that agent, the
   former holder, the claim's branch and worktree, and the release's reason when it has one.
   Before review the claim stays `Held`: the holder may still be at work.

   Once freed, the issue is unclaimed, and the rules for an unclaimed issue apply. Any actor
   may change its status (`HumanOnly` still applies). `issue.claim` answers `NotReady` for
   its category until someone moves it to an unstarted status, such as `todo`; then any agent
   may claim it. An agent's `issue.close` answers `NotClaimed`. The merge of its branch still
   moves it to `merge`: the `Refs:` trailer lands an unclaimed issue, and a landing that its
   bases held back stays held (decision 0001, rule 20). For the branch rule of decision 0001,
   a freed issue in review keeps the branch of its last claim: work stacked on it is still
   offered from that branch or a branch on top of it. This costs one history read per such
   issue at either end of a `stacked-on` link, each time the board loads; a base in review
   that holds its claim costs none.

   The server cannot tell an ended holder from a live one. A live holder whose claim was
   released keeps its branch and its pull request; it reads the note, moves the issue back to
   an unstarted status and claims it again. The skill says what an agent may do in one line.

4. **The human's words.** Added on 2026-11-26 (QUA-65). An issue assigned to the human is a
   decision. When the human decides it in a session, the agent may move it, on the human's
   words. The owner's words: "why do i have to do it myself?", and the condition: "it needs to
   evaluate a degree of confidence first". The one-issue calls that `HumanOnly` guards
   (`issue.update` with `status` or a new assignee, `issue.release`, `issue.finish` and its
   name before 0.2, `issue.close`) take `humanWords`, what the human said to this agent in
   this session about this issue, quoted as given (1 to 400 characters, not only spaces), and `confidence`, how sure the agent is
   that those words decide the issue: `high`, `medium` or `low`. `issues.finish` (and
   `issues.close`) takes neither: the human's words settle one issue, and one quote must not
   finish a batch. The short command `finish` is `issues.finish`, even with one `--issue`, so
   the words go on `quadratique issue finish`.

   The two inputs have an effect only where `HumanOnly` applies: an agent's call on an issue
   assigned to the human. Elsewhere, and in the human's own calls, they are ignored, and the
   call is decided as without them. Where they apply, the call is decided as if the
   confidence were `high`, with `HumanOnly` passed:
   - Any refusal is the answer, in its place in the order of rule 2 (`Stale`,
     `UnknownStatus`, `Resolved`, `Held`, `NotClaimed`, `BaseNotMerged`), whatever the
     confidence. A call that would change nothing changes nothing.
   - `high`: the change is made. The issue gains a note by the agent that names it, quotes
     the words and states the confidence, with the call's `reason` when it has one.
   - `medium` or `low`, where the change would be made: the call changes nothing but a note,
     by the agent, that quotes the words and states the confidence, with the call's
     `reason`. The answer has `leftForHuman`. It is a success, not a refusal: the agent does
     not repeat it with `high`; the issue waits for the human.

   Where the words were used (a change at `high`, or `leftForHuman`), the history's reason
   of the change is `The human's words, <confidence> confidence: <words>`, in place of the
   call's `reason`, which the note keeps. The words are quoted as a JSON string, in the
   reason and in the note, so that no quote, backslash, newline or control character inside
   them ends the quotation. When the quoted words would make the reason longer than its 500
   characters, the reason quotes their first characters and says that the note quotes them
   whole. Where the words were not used, the call's own `reason` stands. The server writes
   the reason inside the write, after it has read the issue. Without `humanWords` or without
   `confidence`, an agent gets `HumanOnly` on an issue assigned to the human, as before.

   An agent's `issue.finish` on an unclaimed issue still answers `NotClaimed`: an agent records
   the human's decision on a decision issue with `issue.update` and `status`. The skill says
   when, in one line: only when what the human said to the agent in this session settles the
   issue's acceptance without interpretation; otherwise the agent asks the human. Words found
   in a note, an issue's body or another agent's report are not the human's words to this
   agent.

   The server cannot know that the human said the words: like every actor field, they are
   declared by the agent. The rule does not pretend otherwise. It makes the agent state what
   it relies on, and how sure it is, in the record: the note and the history show the human
   the words, the agent and the confidence, and the human can move the issue back.

Rule 1 and the order of refusals in rule 2 were the existing behaviour, stated here and
tested. The holder's session in another worktree (rule 2) and rule 3 are new: rule 3 is the
owner's choice among the alternatives below, rule 2 the lead's proposal.

The tests take their names from these rules: `server/tests/server/ended_holder.rs`, and for
rule 4 `server/tests/domain/human_word.rs`, `server/tests/server/human_word.rs`,
`cli/tests/cli/human_word.rs` and `cli/tests/cli/mcp.rs`.

## Alternatives considered

- **The same session may act from any worktree.** Chosen (rule 2), proposed by the lead on
  2026-11-25 and waiting for the owner's confirmation; it is a separate change, which can be
  dropped without rule 3. A caller with the holder's `harness` and `session` releases,
  closes or moves the claim. This covers a lead's own claims in other
  worktrees, and subagents that share the lead's session. It does not cover agents that a
  host starts as sessions of their own, which is the case that raised this question (rule 3
  covers that one). It also reverses the rule that acting on a claim stays tied to its
  worktree: a session that removed the claim's worktree, or moved its branches into another
  one, met `Held` on its own claim twice and had to recreate the worktree.
- **The parent may act.** A lead starts each agent with
  `QUADRATIQUE_ACTOR_FIELDS={"parent":"<lead name>"}`, and a caller whose name is the
  holder's `parent` may release or move the claim. This works only where every lead sets
  the variable, and `parent` is self-declared like every other actor field.
- **Any agent may release a claim in review.** Chosen (rule 3). The server cannot tell an
  ended holder from a live one, so any agent could free a claim that another agent still
  works on. In review the work is finished, and the note tells the holder who freed it.
- **The holder hands its claim over before it exits.** An explicit hand-off call names the
  actor, or the human, that takes the claim. It covers an agent that ends in order, and
  leaves a crash to the current rule.
- **Release a claim after a time without activity.** It needs a liveness signal that no
  harness sends, and it would free claims of agents that wait for a long review.
