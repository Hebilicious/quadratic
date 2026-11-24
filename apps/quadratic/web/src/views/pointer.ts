import type { IssueSummary } from "../board/model.ts";
import { paths } from "../route.ts";
import { useWorkspace } from "../workspace.ts";

export function useIssuePointer(issue: () => string) {
  const { keys, router } = useWorkspace();
  return {
    get href() {
      return paths.issue(issue());
    },
    point: () => {
      keys.send({ type: "point", issue: issue() });
    },
    toggle: () => {
      keys.send({ type: "toggle", issue: issue() });
    },
    open: () => {
      router.go(paths.issue(issue()));
    },
    follow: (event: MouseEvent) => {
      if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      router.go(paths.issue(issue()));
    },
  };
}

export function useStatusName(): (issue: IssueSummary) => string {
  const { board } = useWorkspace();
  return (issue) =>
    board
      .getSnapshot()
      .context.projects.find((project) => project.key === issue.project)
      ?.workflow.statuses.find((status) => status.key === issue.status)?.name ?? issue.status;
}
