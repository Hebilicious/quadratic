import type { IssueSummary, Project } from "./model.ts";
import { isResolved } from "./now.ts";

export function closeChanges(
  issue: Pick<IssueSummary, "category" | "status">,
  project: Project | undefined,
): boolean {
  return (
    project !== undefined &&
    !isResolved(issue.category) &&
    issue.status !== project.workflow.moves.merge
  );
}

export function explainsNotReady(issue: Pick<IssueSummary, "ready" | "category">): boolean {
  return !issue.ready && (issue.category === "backlog" || issue.category === "unstarted");
}
