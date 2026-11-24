import type { Command } from "../board/commands.machine.ts";
import type { IssueSummary, Project } from "../board/model.ts";
import type { IssuePatch } from "../core/quadratique.ts";
import { closeChanges } from "../board/lifecycle.ts";
import {
  ALL,
  GROUPINGS,
  isFiltered,
  NO_FILTER,
  projectScope,
  type Grouping,
} from "../board/view.ts";
import { atPast, paths, withQuery, type Route } from "../route.ts";
import { priorityLabel, PRIORITY_CHOICES } from "../views/present.ts";

export type MenuAction =
  | { readonly kind: "go"; readonly path: string }
  | { readonly kind: "switchView" }
  | { readonly kind: "create" }
  | { readonly kind: "help" }
  | { readonly kind: "search" }
  | { readonly kind: "peek"; readonly issue: string }
  | { readonly kind: "run"; readonly commands: readonly Command[] };

export interface MenuItem {
  readonly id: string;
  readonly group: "Issues" | "Go to" | "Page";
  readonly label: string;
  readonly hint?: string;
  readonly action: MenuAction;
}

export interface MenuInput {
  readonly projects: readonly Project[];
  readonly view: "list" | "board" | "other";
  readonly route: Route;
  readonly targets: readonly IssueSummary[];
  readonly roles: readonly string[];
  readonly query: string;
}

const ROLE = /^[a-z][a-z0-9]*(-[a-z0-9]+)*$/u;

const update = (targets: readonly IssueSummary[], patch: IssuePatch) =>
  targets.map((issue): Command => ({
    kind: "update",
    issue: issue.id,
    revision: issue.revision,
    patch,
  }));

function issueItems(input: MenuInput): MenuItem[] {
  const { targets } = input;
  const [first] = targets;
  if (first === undefined) {
    return [];
  }
  const project = targets.every((issue) => issue.project === first.project)
    ? input.projects.find((each) => each.key === first.project)
    : undefined;
  const what = targets.length === 1 ? first.id : `${String(targets.length)} issues`;
  const items: MenuItem[] = [];
  if (targets.length === 1) {
    items.push(
      {
        id: "open",
        group: "Issues",
        label: `Open ${first.id}`,
        hint: "Enter",
        action: { kind: "go", path: paths.issue(first.id) },
      },
      {
        id: "peek",
        group: "Issues",
        label: `Peek at ${first.id}`,
        hint: "Space",
        action: { kind: "peek", issue: first.id },
      },
    );
  }
  const statuses = project?.workflow.statuses ?? [];
  for (const status of statuses) {
    if (targets.every((issue) => issue.status === status.key)) {
      continue;
    }
    items.push({
      id: `status:${status.key}`,
      group: "Issues",
      label: `Status ${status.name}: move ${what}`,
      action: { kind: "run", commands: update(targets, { status: status.key }) },
    });
  }
  for (const priority of PRIORITY_CHOICES) {
    if (targets.every((issue) => issue.priority === priority)) {
      continue;
    }
    items.push({
      id: `priority:${String(priority)}`,
      group: "Issues",
      label: `Priority ${priorityLabel(priority)}: set on ${what}`,
      action: { kind: "run", commands: update(targets, { priority }) },
    });
  }
  items.push(
    {
      id: "assign:human",
      group: "Issues",
      label: `Assign ${what} to you`,
      action: { kind: "run", commands: update(targets, { assignee: { kind: "human" } }) },
    },
    {
      id: "assign:nobody",
      group: "Issues",
      label: `Assign ${what} to nobody`,
      action: { kind: "run", commands: update(targets, { clear: ["assignee"] }) },
    },
  );
  const typed = /^\s*assign\s+(?<role>\S+)\s*$/iu
    .exec(input.query)
    ?.groups?.["role"]?.toLowerCase();
  const roles =
    typed !== undefined && ROLE.test(typed) && !input.roles.includes(typed)
      ? [...input.roles, typed]
      : input.roles;
  for (const role of roles) {
    items.push({
      id: `assign:role:${role}`,
      group: "Issues",
      label: `Assign ${what} to the role ${role}`,
      action: { kind: "run", commands: update(targets, { assignee: { kind: "agent", role } }) },
    });
  }
  const claimed = targets.filter((issue) => issue.claim !== undefined);
  if (claimed.length > 0) {
    items.push({
      id: "release",
      group: "Issues",
      label: `Release the claim on ${claimed.length === 1 ? (claimed[0]?.id ?? "") : `${String(claimed.length)} issues`}`,
      action: {
        kind: "run",
        commands: claimed.map((issue): Command => ({ kind: "release", issue: issue.id })),
      },
    });
  }
  if (targets.length === 1 && closeChanges(first, project)) {
    items.push({
      id: "close",
      group: "Issues",
      label: `Close ${first.id} with evidence…`,
      action: { kind: "go", path: paths.closing(first.id) },
    });
  }
  return items;
}

export const GROUPING_NAMES: Readonly<Record<Grouping, string>> = {
  status: "status",
  priority: "priority",
  label: "label",
  project: "project",
  milestone: "milestone",
  parent: "parent issue",
};

function viewItems(route: Route): MenuItem[] {
  if (route.view !== "list" && route.view !== "board") {
    return [];
  }
  const { query } = route;
  const items: MenuItem[] = GROUPINGS.filter((grouping) => grouping !== query.group).map(
    (grouping) => ({
      id: `group:${grouping}`,
      group: "Page",
      label: `Group by ${GROUPING_NAMES[grouping]}`,
      action: { kind: "go", path: withQuery(route, { ...query, group: grouping }) },
    }),
  );
  items.push({
    id: "search",
    group: "Page",
    label: "Search the issues shown",
    hint: "/",
    action: { kind: "search" },
  });
  if (query.filter.ready !== true) {
    items.push({
      id: "filter:ready",
      group: "Page",
      label: "Show only the ready issues",
      action: {
        kind: "go",
        path: withQuery(route, { ...query, filter: { ...query.filter, ready: true } }),
      },
    });
  }
  if (isFiltered(query.filter)) {
    items.push({
      id: "filter:clear",
      group: "Page",
      label: "Clear the filters",
      action: { kind: "go", path: withQuery(route, { ...query, filter: NO_FILTER }) },
    });
  }
  if (query.at !== undefined) {
    items.push({
      id: "moment:now",
      group: "Page",
      label: "Back to now",
      action: { kind: "go", path: withQuery(route, { ...query, at: undefined }) },
    });
  }
  return items;
}

export function menuItems(input: MenuInput): readonly MenuItem[] {
  const places: MenuItem[] = [
    {
      id: "go:waiting",
      group: "Go to",
      label: "Go to what waits on you",
      hint: "G I",
      action: { kind: "go", path: paths.waiting() },
    },
    {
      id: "go:now",
      group: "Go to",
      label: "Go to what every agent is doing",
      hint: "G M",
      action: { kind: "go", path: paths.now() },
    },
    {
      id: "go:history",
      group: "Go to",
      label: "Go to the history of changes",
      hint: "G H",
      action: {
        kind: "go",
        path: paths.history(
          input.route.view === "list" ||
            input.route.view === "board" ||
            input.route.view === "history"
            ? input.route.scope
            : ALL,
        ),
      },
    },
    ...(input.projects.length === 0
      ? []
      : [
          {
            id: "go:list:all",
            group: "Go to",
            label: "Go to all projects list",
            action: { kind: "go", path: paths.list(ALL) },
          } satisfies MenuItem,
          {
            id: "go:board:all",
            group: "Go to",
            label: "Go to all projects board",
            action: { kind: "go", path: paths.board(ALL) },
          } satisfies MenuItem,
        ]),
    ...input.projects.flatMap((project): MenuItem[] => [
      {
        id: `go:list:${project.key}`,
        group: "Go to",
        label: `Go to ${project.name} (${project.key}) list`,
        action: { kind: "go", path: paths.list(projectScope(project.key)) },
      },
      {
        id: `go:board:${project.key}`,
        group: "Go to",
        label: `Go to ${project.name} (${project.key}) board`,
        action: { kind: "go", path: paths.board(projectScope(project.key)) },
      },
    ]),
  ];
  const page: MenuItem[] = [
    ...(input.view === "other"
      ? []
      : [
          {
            id: "switch",
            group: "Page",
            label: input.view === "list" ? "Switch to the board" : "Switch to the list",
            hint: "Mod B",
            action: { kind: "switchView" },
          } satisfies MenuItem,
        ]),
    ...viewItems(input.route),
    ...(input.projects.length === 0 || atPast(input.route)
      ? []
      : [
          {
            id: "create",
            group: "Page",
            label: "Create an issue",
            hint: "C",
            action: { kind: "create" },
          } satisfies MenuItem,
        ]),
    {
      id: "help",
      group: "Page",
      label: "Show the keyboard shortcuts",
      hint: "?",
      action: { kind: "help" },
    },
  ];
  return [...issueItems(input), ...page, ...places];
}

export function matching(items: readonly MenuItem[], query: string): readonly MenuItem[] {
  const words = query
    .toLowerCase()
    .split(/\s+/u)
    .filter((word) => word.length > 0);
  return items.filter((item) => {
    const label = item.label.toLowerCase();
    return words.every((word) => label.includes(word));
  });
}
