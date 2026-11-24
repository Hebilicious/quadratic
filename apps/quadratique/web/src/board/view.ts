import type { IssuePatch } from "../core/quadratique.ts";
import { adjacentStatus, type Issues } from "./issues.ts";
import type { Category, IssueSummary, Project, WorkflowStatus } from "./model.ts";

export type Scope = { readonly kind: "project"; readonly key: string } | { readonly kind: "all" };

export const ALL: Scope = { kind: "all" };

export const projectScope = (key: string): Scope => ({ kind: "project", key });

export const inScope = (scope: Scope, issue: IssueSummary): boolean =>
  scope.kind === "all" || issue.project === scope.key;

export const sameScope = (left: Scope | undefined, right: Scope | undefined): boolean =>
  left?.kind === right?.kind &&
  (left?.kind !== "project" || right?.kind !== "project" || left.key === right.key);

export type Grouping = "status" | "priority" | "label" | "project" | "milestone" | "parent";

export const GROUPINGS: readonly Grouping[] = [
  "status",
  "priority",
  "label",
  "project",
  "milestone",
  "parent",
];

export const isGrouping = (value: string): value is Grouping =>
  GROUPINGS.some((grouping) => grouping === value);

export type AssigneeFilter =
  | { readonly kind: "nobody" }
  | { readonly kind: "human" }
  | { readonly kind: "role"; readonly role: string };

export type ClaimFilter = "mine" | "any" | "none";

export const CLAIM_FILTERS: readonly ClaimFilter[] = ["mine", "any", "none"];

export interface Filter {
  readonly statuses: readonly string[];
  readonly categories: readonly Category[];
  readonly priorities: readonly number[];
  readonly labels: readonly string[];
  readonly assignee: AssigneeFilter | undefined;
  readonly claim: ClaimFilter | undefined;
  readonly ready: boolean | undefined;
  readonly text: string;
}

export const NO_FILTER: Filter = {
  statuses: [],
  categories: [],
  priorities: [],
  labels: [],
  assignee: undefined,
  claim: undefined,
  ready: undefined,
  text: "",
};

export function isFiltered(filter: Filter): boolean {
  return (
    filter.statuses.length > 0 ||
    filter.categories.length > 0 ||
    filter.priorities.length > 0 ||
    filter.labels.length > 0 ||
    filter.assignee !== undefined ||
    filter.claim !== undefined ||
    filter.ready !== undefined ||
    filter.text.trim().length > 0
  );
}

const wordsOf = (text: string): readonly string[] =>
  text
    .toLowerCase()
    .split(/\s+/u)
    .filter((word) => word.length > 0);

function assigneeMatches(filter: AssigneeFilter, issue: IssueSummary): boolean {
  switch (filter.kind) {
    case "nobody":
      return issue.assignee === undefined;
    case "human":
      return issue.assignee?.kind === "human";
    case "role":
      return issue.assignee?.kind === "agent" && issue.assignee.role === filter.role;
    default: {
      const unhandled: never = filter;
      return unhandled;
    }
  }
}

function claimMatches(filter: ClaimFilter, issue: IssueSummary): boolean {
  switch (filter) {
    case "any":
      return issue.claim !== undefined;
    case "none":
      return issue.claim === undefined;
    case "mine":
      return issue.claim?.holder.kind === "human";
    default: {
      const unhandled: never = filter;
      return unhandled;
    }
  }
}

export function matches(filter: Filter, issue: IssueSummary): boolean {
  const statusNamed = filter.statuses.length > 0 || filter.categories.length > 0;
  const words = wordsOf(filter.text);
  const searched = `${issue.id} ${issue.title}`.toLowerCase();
  return (
    (!statusNamed ||
      filter.statuses.includes(issue.status) ||
      filter.categories.includes(issue.category)) &&
    (filter.priorities.length === 0 || filter.priorities.includes(issue.priority)) &&
    filter.labels.every((label) => issue.labels.includes(label)) &&
    (filter.assignee === undefined || assigneeMatches(filter.assignee, issue)) &&
    (filter.claim === undefined || claimMatches(filter.claim, issue)) &&
    (filter.ready === undefined || issue.ready === filter.ready) &&
    words.every((word) => searched.includes(word))
  );
}

export type Move =
  | { readonly kind: "status"; readonly status: string }
  | { readonly kind: "priority"; readonly priority: number }
  | { readonly kind: "milestone"; readonly milestone: string | undefined }
  | { readonly kind: "parent"; readonly parent: string | undefined };

export type Tone =
  | { readonly kind: "category"; readonly category: Category }
  | { readonly kind: "priority"; readonly priority: number }
  | { readonly kind: "plain" };

export interface Group {
  readonly key: string;
  readonly name: string;
  readonly tone: Tone;
  readonly move: Move | undefined;
  readonly issues: readonly IssueSummary[];
}

export interface GroupingContext {
  readonly projects: readonly Project[];
  readonly scope: Scope;
  readonly unfiltered: Issues;
  readonly milestones: Readonly<Record<string, string | null>>;
}

const CATEGORY_ORDER: readonly Category[] = [
  "backlog",
  "unstarted",
  "started",
  "completed",
  "canceled",
];

export const PRIORITY_ORDER: readonly number[] = [1, 2, 3, 4, 0];

const PRIORITY_NAMES: Readonly<Record<number, string>> = {
  0: "No priority",
  1: "Urgent",
  2: "High",
  3: "Medium",
  4: "Low",
};

const byOrder = (left: IssueSummary, right: IssueSummary): number =>
  left.order - right.order || left.id.localeCompare(right.id);

export function statusesOf(projects: readonly Project[], scope: Scope): readonly WorkflowStatus[] {
  const seen = new Map<string, WorkflowStatus>();
  for (const project of projects) {
    if (scope.kind === "project" && project.key !== scope.key) {
      continue;
    }
    for (const status of project.workflow.statuses) {
      if (!seen.has(status.key)) {
        seen.set(status.key, status);
      }
    }
  }
  return [...seen.values()]
    .map((status, index) => ({ status, index }))
    .toSorted(
      (left, right) =>
        CATEGORY_ORDER.indexOf(left.status.category) -
          CATEGORY_ORDER.indexOf(right.status.category) || left.index - right.index,
    )
    .map(({ status }) => status);
}

const NONE = "\u0000none";

function bucketed(
  issues: readonly IssueSummary[],
  valuesOf: (issue: IssueSummary) => readonly string[],
): Map<string, IssueSummary[]> {
  const buckets = new Map<string, IssueSummary[]>();
  for (const issue of issues.toSorted(byOrder)) {
    const values = valuesOf(issue);
    for (const value of values.length === 0 ? [NONE] : values) {
      const bucket = buckets.get(value) ?? [];
      bucket.push(issue);
      buckets.set(value, bucket);
    }
  }
  return buckets;
}

export function grouped(
  grouping: Grouping,
  issues: readonly IssueSummary[],
  context: GroupingContext,
): readonly Group[] {
  switch (grouping) {
    case "status": {
      const buckets = bucketed(issues, (issue) => [issue.status]);
      const statuses = statusesOf(context.projects, context.scope);
      const known = new Set(statuses.map((status) => status.key));
      const unknown = [...buckets.entries()]
        .filter(([key]) => !known.has(key))
        .toSorted(([left], [right]) => left.localeCompare(right))
        .map(([key, members]): Group => ({
          key,
          name: key,
          tone: { kind: "category", category: members[0]?.category ?? "backlog" },
          move: undefined,
          issues: members,
        }));
      return statuses
        .map((status): Group => ({
          key: status.key,
          name: status.name,
          tone: { kind: "category", category: status.category },
          move: { kind: "status", status: status.key },
          issues: buckets.get(status.key) ?? [],
        }))
        .concat(unknown);
    }
    case "priority": {
      const buckets = bucketed(issues, (issue) => [String(issue.priority)]);
      return PRIORITY_ORDER.map((priority) => ({
        key: String(priority),
        name: PRIORITY_NAMES[priority] ?? `Priority ${String(priority)}`,
        tone: { kind: "priority", priority },
        move: { kind: "priority", priority },
        issues: buckets.get(String(priority)) ?? [],
      }));
    }
    case "label": {
      const buckets = bucketed(issues, (issue) => issue.labels);
      return [...buckets.keys()]
        .filter((label) => label !== NONE)
        .toSorted()
        .map((label): Group => plain(label, label, undefined, buckets.get(label) ?? []))
        .concat(noneGroup("No label", undefined, buckets));
    }
    case "project": {
      const buckets = bucketed(issues, (issue) => [issue.project]);
      return context.projects
        .filter((project) => buckets.has(project.key))
        .map((project) =>
          plain(project.key, project.name, undefined, buckets.get(project.key) ?? []),
        );
    }
    case "milestone": {
      const buckets = bucketed(issues, (issue) => {
        const milestone = context.milestones[issue.id];
        return milestone === undefined || milestone === null ? [] : [milestone];
      });
      return [...buckets.keys()]
        .filter((milestone) => milestone !== NONE)
        .toSorted()
        .map((milestone): Group =>
          plain(
            milestone,
            milestone,
            { kind: "milestone", milestone },
            buckets.get(milestone) ?? [],
          ),
        )
        .concat(
          noneGroup("No milestone", { kind: "milestone", milestone: undefined }, buckets, true),
        );
    }
    case "parent": {
      const buckets = bucketed(issues, (issue) =>
        issue.parent === undefined ? [] : [issue.parent],
      );
      const parentOrder = (id: string) => context.unfiltered[id]?.order ?? Number.POSITIVE_INFINITY;
      return [...buckets.keys()]
        .filter((parent) => parent !== NONE)
        .toSorted(
          (left, right) => parentOrder(left) - parentOrder(right) || left.localeCompare(right),
        )
        .map((parent): Group => {
          const title = context.unfiltered[parent]?.title;
          return plain(
            parent,
            title === undefined ? parent : `${parent} ${title}`,
            { kind: "parent", parent },
            buckets.get(parent) ?? [],
          );
        })
        .concat(noneGroup("No parent", { kind: "parent", parent: undefined }, buckets, true));
    }
    default: {
      const unhandled: never = grouping;
      return unhandled;
    }
  }
}

function plain(
  key: string,
  name: string,
  move: Move | undefined,
  issues: readonly IssueSummary[],
): Group {
  return { key, name, tone: { kind: "plain" }, move, issues };
}

function noneGroup(
  name: string,
  move: Move | undefined,
  buckets: Map<string, IssueSummary[]>,
  keepEmpty = false,
): readonly Group[] {
  const issues = buckets.get(NONE) ?? [];
  return issues.length > 0 || keepEmpty ? [plain(NONE, name, move, issues)] : [];
}

export function movable(grouping: Grouping): boolean {
  return (
    grouping === "status" ||
    grouping === "priority" ||
    grouping === "milestone" ||
    grouping === "parent"
  );
}

export function patchOfMove(
  move: Move,
  issue: IssueSummary,
  context: Pick<GroupingContext, "projects" | "milestones">,
): IssuePatch | undefined {
  switch (move.kind) {
    case "status": {
      const workflow = context.projects.find((project) => project.key === issue.project)?.workflow;
      const known = workflow?.statuses.some((status) => status.key === move.status) === true;
      return known && issue.status !== move.status ? { status: move.status } : undefined;
    }
    case "priority":
      return issue.priority === move.priority ? undefined : { priority: move.priority };
    case "milestone": {
      const current = context.milestones[issue.id] ?? undefined;
      if (current === move.milestone) {
        return undefined;
      }
      return move.milestone === undefined
        ? { clear: ["milestone"] }
        : { milestone: move.milestone };
    }
    case "parent":
      if (issue.parent === move.parent || issue.id === move.parent) {
        return undefined;
      }
      return move.parent === undefined ? { clear: ["parent"] } : { parent: move.parent };
    default: {
      const unhandled: never = move;
      return unhandled;
    }
  }
}

export function patched(issue: IssueSummary, patches: readonly IssuePatch[]): IssueSummary {
  return patches.reduce((each, patch): IssueSummary => {
    const cleared = patch.clear?.includes("parent") === true;
    const { parent: _parent, ...rest } = each;
    return {
      ...(cleared ? rest : each),
      ...(patch.status === undefined ? {} : { status: patch.status }),
      ...(patch.priority === undefined ? {} : { priority: patch.priority }),
      ...(patch.parent === undefined ? {} : { parent: patch.parent }),
    };
  }, issue);
}

export function patchedMilestone(
  milestone: string | null | undefined,
  patches: readonly IssuePatch[],
): string | null | undefined {
  return patches.reduce(
    (each, patch) => patch.milestone ?? (patch.clear?.includes("milestone") === true ? null : each),
    milestone,
  );
}

function holds(
  move: Move,
  issue: IssueSummary,
  milestones: GroupingContext["milestones"],
): boolean {
  switch (move.kind) {
    case "status":
      return issue.status === move.status;
    case "priority":
      return issue.priority === move.priority;
    case "milestone":
      return (milestones[issue.id] ?? undefined) === move.milestone;
    case "parent":
      return issue.parent === move.parent;
    default: {
      const unhandled: never = move;
      return unhandled;
    }
  }
}

export function shiftPatch(
  grouping: Grouping,
  groups: readonly Pick<Group, "move">[],
  issue: IssueSummary,
  by: -1 | 1,
  context: Pick<GroupingContext, "projects" | "milestones">,
): IssuePatch | undefined {
  if (grouping === "status") {
    const workflow = context.projects.find((project) => project.key === issue.project)?.workflow;
    const next =
      workflow === undefined ? undefined : adjacentStatus(workflow.statuses, issue.status, by);
    return next === undefined ? undefined : { status: next.key };
  }
  if (!movable(grouping)) {
    return undefined;
  }
  const at = groups.findIndex(
    (group) => group.move !== undefined && holds(group.move, issue, context.milestones),
  );
  const move = at === -1 ? undefined : groups[at + by]?.move;
  return move === undefined ? undefined : patchOfMove(move, issue, context);
}

export interface ShiftPlan {
  readonly grouping: Grouping;
  readonly groups: readonly Pick<Group, "move">[];
  readonly projects: readonly Project[];
  readonly issue: IssueSummary;
  readonly milestone: string | null | undefined;
}

export function shiftPlan(
  grouping: Grouping,
  groups: readonly Group[],
  issue: IssueSummary,
  context: Pick<GroupingContext, "projects" | "milestones">,
): ShiftPlan {
  return {
    grouping,
    groups: groups.map((group) => ({ move: group.move })),
    projects: context.projects.filter((project) => project.key === issue.project),
    issue,
    milestone: context.milestones[issue.id],
  };
}

export function plannedPatch(
  plan: ShiftPlan,
  by: -1 | 1,
  pending: readonly IssuePatch[],
): IssuePatch | undefined {
  const issue = patched(plan.issue, pending);
  const milestone = patchedMilestone(plan.milestone, pending);
  return shiftPatch(plan.grouping, plan.groups, issue, by, {
    projects: plan.projects,
    milestones: milestone === undefined ? {} : { [issue.id]: milestone },
  });
}

export function cursorGroups(groups: readonly Group[]): ReadonlyMap<string, string> {
  const first = new Map<string, string>();
  for (const group of groups) {
    for (const issue of group.issues) {
      if (!first.has(issue.id)) {
        first.set(issue.id, group.key);
      }
    }
  }
  return first;
}

export function columnsOf(groups: readonly Group[]): readonly (readonly string[])[] {
  const first = cursorGroups(groups);
  return groups.map((group) =>
    group.issues.filter((issue) => first.get(issue.id) === group.key).map((issue) => issue.id),
  );
}

export function shownGroups(input: {
  readonly grouping: Grouping;
  readonly filter: Filter;
  readonly scope: Scope;
  readonly projects: readonly Project[];
  readonly issues: Issues;
  readonly milestones: Readonly<Record<string, string | null>>;
}): readonly Group[] {
  const passing = Object.values(input.issues).filter(
    (issue) => inScope(input.scope, issue) && matches(input.filter, issue),
  );
  return grouped(input.grouping, passing, {
    projects: input.projects,
    scope: input.scope,
    unfiltered: input.issues,
    milestones: input.milestones,
  });
}
