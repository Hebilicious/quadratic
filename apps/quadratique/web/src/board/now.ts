import { applied, type Issues, type Update } from "./issues.ts";
import type { Actor, Category, IssueSummary, Project } from "./model.ts";

const RESOLVED: readonly Category[] = ["completed", "canceled"];

export const isResolved = (category: Category): boolean => RESOLVED.includes(category);

export const UNRESOLVED: readonly Category[] = ["backlog", "unstarted", "started"];

export function appliedToNow(now: Issues, update: Update, since: string | undefined): Issues {
  const keepIssuesThatLeftTheViewAtTheirLastRevision = () => true;
  return applied(now, update, { since, keep: keepIssuesThatLeftTheViewAtTheirLastRevision });
}

export interface AgentWork {
  readonly holder: Actor;
  readonly worktree: string;
  readonly issues: readonly IssueSummary[];
}

export interface NowView {
  readonly agents: readonly AgentWork[];
  readonly assigned: readonly IssueSummary[];
  readonly review: readonly IssueSummary[];
}

const bySince = (left: IssueSummary, right: IssueSummary): number =>
  (left.claim?.since ?? "").localeCompare(right.claim?.since ?? "");

const byPriorityThenOrder = (left: IssueSummary, right: IssueSummary): number =>
  rank(left.priority) - rank(right.priority) ||
  left.order - right.order ||
  left.id.localeCompare(right.id);

const rank = (priority: number): number => (priority === 0 ? 5 : priority);

export function nowView(now: Issues, projects: readonly Project[]): NowView {
  const reviewOf = new Map(projects.map((project) => [project.key, project.workflow.moves.review]));
  const all = Object.values(now);
  const holders = new Map<string, { holder: Actor; worktree: string; issues: IssueSummary[] }>();
  for (const issue of all.toSorted(bySince)) {
    if (issue.claim === undefined) {
      continue;
    }
    const { holder, worktree } = issue.claim;
    const key = JSON.stringify([holder.name, worktree]);
    const known = holders.get(key) ?? { holder, worktree, issues: [] };
    known.issues.push(issue);
    holders.set(key, known);
  }
  const inReview = (issue: IssueSummary) => reviewOf.get(issue.project) === issue.status;
  return {
    agents: [...holders.values()],
    assigned: all
      .filter((issue) => issue.assignee?.kind === "human" && !isResolved(issue.category))
      .filter((issue) => !inReview(issue))
      .toSorted(byPriorityThenOrder),
    review: all.filter(inReview).toSorted(byPriorityThenOrder),
  };
}

export function waitingCount(view: NowView): number {
  return view.assigned.length + view.review.length;
}
