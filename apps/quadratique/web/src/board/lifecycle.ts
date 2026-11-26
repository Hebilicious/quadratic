import type { IssueSummary, NotReadyReason, Project } from "./model.ts";
import { isResolved } from "./now.ts";

export function finishChanges(
  issue: Pick<IssueSummary, "category" | "status">,
  project: Project | undefined,
): boolean {
  return (
    project !== undefined &&
    !isResolved(issue.category) &&
    issue.status !== project.workflow.moves.merge
  );
}

type Readiness = Pick<IssueSummary, "ready" | "category" | "notReady">;

export function notReadyShown(issue: Readiness): readonly NotReadyReason[] {
  if (issue.ready) {
    return [];
  }
  const reasons = issue.notReady ?? [];
  const unstarted = issue.category === "backlog" || issue.category === "unstarted";
  return unstarted ? reasons : reasons.filter((reason) => reason === "stacked");
}

export function explainsNotReady(issue: Readiness): boolean {
  return notReadyShown(issue).length > 0;
}
