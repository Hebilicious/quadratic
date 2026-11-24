import type { IssueSummary, WorkflowStatus } from "./model.ts";

export type Issues = Readonly<Record<string, IssueSummary>>;

export function issuesById(items: readonly IssueSummary[]): Issues {
  return Object.fromEntries(items.map((issue) => [issue.id, issue]));
}

export type Update =
  | { readonly from: "change"; readonly at: string; readonly issues: readonly IssueSummary[] }
  | { readonly from: "answer"; readonly issues: readonly IssueSummary[] };

type Replaces = (incoming: IssueSummary, held: IssueSummary) => boolean;

const sameRevisionOrNewer: Replaces = (incoming, held) => incoming.revision >= held.revision;
const newerRevisionOnly: Replaces = (incoming, held) => incoming.revision > held.revision;

const replacesHeldSummary: Readonly<Record<Update["from"], Replaces>> = {
  change: sameRevisionOrNewer,
  answer: newerRevisionOnly,
};

export function applied(
  issues: Issues,
  update: Update,
  options: { readonly since: string | undefined; readonly keep: (issue: IssueSummary) => boolean },
): Issues {
  if (update.from === "change" && options.since !== undefined && update.at <= options.since) {
    return issues;
  }
  const next: Record<string, IssueSummary> = { ...issues };
  for (const issue of update.issues) {
    const held = next[issue.id];
    const replaces = held === undefined || replacesHeldSummary[update.from](issue, held);
    if (replaces && options.keep(issue)) {
      next[issue.id] = issue;
    }
  }
  return next;
}

export function adjacentStatus(
  statuses: readonly WorkflowStatus[],
  status: string,
  by: -1 | 1,
): WorkflowStatus | undefined {
  const at = statuses.findIndex((each) => each.key === status);
  return at === -1 ? undefined : statuses[at + by];
}

export function rolesOf(...sources: readonly Issues[]): readonly string[] {
  const roles = new Set<string>();
  for (const issues of sources) {
    for (const issue of Object.values(issues)) {
      if (issue.assignee?.kind === "agent") {
        roles.add(issue.assignee.role);
      }
      if (issue.claim?.holder.role !== undefined) {
        roles.add(issue.claim.holder.role);
      }
    }
  }
  return [...roles].toSorted();
}
