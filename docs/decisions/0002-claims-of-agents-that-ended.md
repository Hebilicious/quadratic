# 0002. Claims of agents that ended

Date: 2026-11-25

## Context

A lead agent starts other agents. One of them claims an issue, closes it into review and ends.
The lead, or a reviewer, then needs to add a note to that issue, or to free it when the work
will not be merged (an audit, for example).

The rules are in `Rules` in `server/src/domain/lifecycle.rs`. The holder of a claim is the
same actor name in the same worktree (`Caller::holds` in `server/src/domain/issue.rs`).

The server knows of an agent only what the agent's actor says (`server/src/domain/actor.rs`):

- `session` comes from the harness: `CLAUDE_CODE_SESSION_ID` for Claude Code,
  `CODEX_THREAD_ID` for Codex.
- `host` and `hostThread` come from the host: `T3CODE_THREAD_ID` for T3 Code.
- `parent` comes only from `QUADRATIQUE_ACTOR_FIELDS`. No harness or host sets it.

An agent that a T3 Code thread delegates to has its own session id. Its `hostThread` is an
opaque id of the delegated task, which does not contain the lead's thread or session. So
nothing the server receives links that agent to the lead that started it. The server also
cannot tell whether a holder has ended: a claim records no liveness.

## Decision

1. **Notes.** Any actor may add a note to any issue, claimed or not, in any status:
   `issue.update` with `notes` and the issue's current `revision`. A review note reaches an
   issue in review this way, from a human or from any agent. Fields without a status
   (`description`, `acceptance`, `priority`, `notes`) are open to every actor.
2. **Moves.** Only the holder or the human may change the status of a claimed issue, release
   it or close it. Any other agent gets `Held`, which names the holder. This includes the
   holder's own session when it works in another worktree.
3. **A claim whose holder ended.** The lead adds what it knows as a note and asks the human
   to release the claim or to move the issue. The skill says so in one line.

Rules 1 and 2 are the existing behaviour, stated here and tested. Rule 3 keeps the existing
behaviour until the maintainer chooses one of the alternatives below: which agents may free
another agent's claim is a product decision, and the code does not settle it.

The tests take their names from these rules: `server/tests/server/ended_holder.rs`.

## Alternatives considered

- **The same session may act from any worktree.** A caller with the holder's `harness` and
  `session` could release or move the claim. This covers a lead's own claims in other
  worktrees, and subagents that share the lead's session. It does not cover agents that a
  host starts as sessions of their own, which is the case that raised this question. It
  also reverses the rule that acting on a claim stays tied to its worktree.
- **The parent may act.** A lead starts each agent with
  `QUADRATIQUE_ACTOR_FIELDS={"parent":"<lead name>"}`, and a caller whose name is the
  holder's `parent` may release or move the claim. This works only where every lead sets
  the variable, and `parent` is self-declared like every other actor field.
- **Any agent may release a claim in review.** The server cannot tell an ended holder from a
  live one, so any agent could take a claim that another agent is still working on.
- **Release a claim after a time without activity.** It needs a liveness signal that no
  harness sends, and it would free claims of agents that wait for a long review.
