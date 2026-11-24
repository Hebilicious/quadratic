import { Option, Schema } from "effect";

import {
  FailureSchema,
  type Actor,
  type Assignee,
  type FieldChange,
  type NotReadyReason,
  type Relation,
  type RelationKind,
} from "../board/model.ts";
import { PastMoment, Refused } from "../core/quadratic.ts";

const PRIORITIES = ["No priority", "Urgent", "High", "Medium", "Low"] as const;

export function priorityLabel(priority: number): string {
  return PRIORITIES[priority] ?? `Priority ${String(priority)}`;
}

export function assigneeLabel(assignee: Assignee | undefined): string | undefined {
  if (assignee === undefined) {
    return undefined;
  }
  return assignee.kind === "human" ? "You" : assignee.role;
}

export function worktreeName(worktree: string): string {
  return worktree.replace(/\/+$/u, "").split("/").at(-1) ?? worktree;
}

export function worktreeBesideName(holder: string, worktree: string): string | undefined {
  const name = worktreeName(worktree);
  return holder.endsWith(`@${name}`) ? undefined : name;
}

const decodeFailure = Schema.decodeUnknownOption(FailureSchema);

export function failureText(error: unknown): string {
  if (error instanceof PastMoment) {
    return "Nothing can be changed on the board as it was. Go back to now to change it.";
  }
  if (!(error instanceof Refused)) {
    return "The server could not be reached, or answered in a way the page does not understand. Try again.";
  }
  const failure = decodeFailure(error.failure);
  if (Option.isNone(failure)) {
    return "The server refused the change.";
  }
  const known = failure.value;
  switch (known._tag) {
    case "Stale":
      return "The issue changed since it was read, so nothing was saved. It now shows as it is; what you entered is kept. Save again to apply it.";
    case "Held":
      return `${known.claim.holder.name} holds ${known.issue} on ${known.claim.branch}; only the holder or you can change its status. Release the claim first.`;
    case "Cycle":
      return `That would make issues wait on each other: ${known.path.join(" → ")}.`;
    case "HumanOnly":
      return "Only the human can do that.";
    case "NotFound":
      return `There is no ${known.kind} ${known.id}.`;
    case "UnknownStatus":
      return `The status ${known.status} is not in the workflow (${known.statuses.join(", ")}).`;
    case "SelfLink":
      return "An issue cannot be related to itself.";
    case "NotClaimed":
      return `Nobody holds ${known.issue}.`;
    case "Resolved":
      return `${known.issue} is already resolved (${known.status}).`;
    case "KeyConflict":
      return `This form was already sent with other values, as ${known.issue}.`;
    case "NoProject":
      return "Choose a project for the issue.";
    case "UnknownCommit":
      return `No single database commit has the hash ${known.commit}.`;
    default: {
      const unhandled: never = known;
      return unhandled;
    }
  }
}

const NOT_READY: Readonly<Record<NotReadyReason, string>> = {
  category: "Its status is not one where work starts.",
  blocked: "It waits on an unresolved blocker, of its own or of a parent.",
  children: "It waits on unresolved children.",
  assigned: "It is assigned to an agent role, not to you.",
  claimed: "Someone holds it.",
  mine: "You hold it.",
};

export function notReadyText(reason: NotReadyReason): string {
  return NOT_READY[reason];
}

const RELATION_OUT: Readonly<Record<RelationKind, string>> = {
  blocks: "Blocks",
  related: "Related to",
  duplicate: "Duplicate of",
  "discovered-from": "Discovered from",
};

const RELATION_IN: Readonly<Record<RelationKind, string>> = {
  blocks: "Blocked by",
  related: "Related to",
  duplicate: "Duplicated by",
  "discovered-from": "Led to",
};

export function relationLabel(relation: Pick<Relation, "kind" | "direction">): string {
  return relation.direction === "outgoing"
    ? RELATION_OUT[relation.kind]
    : RELATION_IN[relation.kind];
}

export const RELATION_KINDS: readonly RelationKind[] = [
  "blocks",
  "related",
  "duplicate",
  "discovered-from",
];

export function relationKindLabel(kind: RelationKind): string {
  return RELATION_OUT[kind];
}

export function actorDetails(actor: Actor): readonly string[] {
  return [
    ...(actor.role === undefined ? [] : [`role ${actor.role}`]),
    ...(actor.harness === undefined ? [] : [`harness ${actor.harness}`]),
    ...(actor.host === undefined ? [] : [`host ${actor.host}`]),
  ];
}

const relative = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
const STEPS: readonly [Intl.RelativeTimeFormatUnit, number][] = [
  ["second", 60],
  ["minute", 60],
  ["hour", 24],
  ["day", 30],
  ["month", 12],
  ["year", Number.POSITIVE_INFINITY],
];

export function ago(moment: string, now: number = Date.now()): string {
  let amount = (Date.parse(moment) - now) / 1000;
  for (const [unit, size] of STEPS) {
    if (Math.abs(amount) < size) {
      return relative.format(Math.round(amount), unit);
    }
    amount /= size;
  }
  return moment;
}

export function when(moment: string): string {
  return new Date(moment).toLocaleString();
}

const OPERATIONS: Readonly<Record<string, string>> = {
  "issue.create": "created",
  "issue.update": "changed",
  "issue.claim": "claimed",
  "issue.release": "released the claim on",
  "issue.close": "closed with evidence",
  "issue.link": "changed the relations of",
  "git.merged": "saw merged the work of",
  "findings.import": "imported a tool's findings into",
  "project.create": "created the project",
  "project.update": "changed the project",
};

export function operationText(operation: string): string {
  return OPERATIONS[operation] ?? operation;
}

const FIELD_NAMES: Readonly<Record<string, string>> = {
  title: "Title",
  status: "Status",
  priority: "Priority",
  assignee: "Assignee",
  parent: "Parent",
  milestone: "Milestone",
  due: "Due",
  description: "Description",
  acceptance: "Acceptance criteria",
  notes: "Notes",
  evidence: "Evidence",
  claim: "Claim",
  labels: "Labels",
  relations: "Relation",
  source: "Source",
  links: "Links",
  decision: "Decision",
  name: "Name",
  workflow: "Workflow",
};

const BOOKKEEPING: ReadonlySet<string> = new Set([
  "project",
  "order",
  "revision",
  "createdBy",
  "createdAt",
  "updatedAt",
]);

const parsed = (json: string): unknown => {
  try {
    return JSON.parse(json);
  } catch {
    return json;
  }
};

const member = (value: unknown, key: string): unknown =>
  typeof value === "object" && value !== null && key in value
    ? Object.entries(value).find(([name]) => name === key)?.[1]
    : undefined;

const text = (value: unknown): string =>
  typeof value === "string" ? value : value === null ? "nothing" : JSON.stringify(value);

function valueText(field: string, value: unknown, statusName: (key: string) => string): string {
  switch (field) {
    case "status":
      return statusName(text(value));
    case "priority":
      return typeof value === "number" ? priorityLabel(value) : text(value);
    case "assignee":
      return member(value, "kind") === "human" ? "you" : `role ${text(member(value, "role"))}`;
    case "claim":
      return `${text(member(member(value, "holder"), "name"))} on ${text(member(value, "branch"))}`;
    case "labels":
      return Array.isArray(value) ? value.map(text).join(", ") || "none" : text(value);
    case "relations": {
      const kind = member(value, "kind");
      const direction = member(value, "direction");
      const known = RELATION_KINDS.find((each) => each === kind);
      return known === undefined || (direction !== "outgoing" && direction !== "incoming")
        ? text(value)
        : `${relationLabel({ kind: known, direction })} ${text(member(value, "issue"))}`;
    }
    default:
      return text(value);
  }
}

export interface ChangeLine {
  readonly field: string;
  readonly label: string;
  readonly before: string | undefined;
  readonly after: string | undefined;
  readonly long: boolean;
  readonly exact: boolean;
}

const LONG = 80;

const collapsed = (value: string): string => value.replace(/\s+/gu, " ").trim();

const escaped = (value: unknown): string =>
  JSON.stringify(value).replace(
    /\s/gu,
    (space) => `\\u${space.charCodeAt(0).toString(16).padStart(4, "0")}`,
  );

function changeLine(change: FieldChange, statusName: (key: string) => string): ChangeLine {
  const stored = (json: string | undefined) => (json === undefined ? undefined : parsed(json));
  const words = (json: string | undefined) =>
    json === undefined ? undefined : valueText(change.field, parsed(json), statusName);
  const [storedBefore, storedAfter] = [stored(change.before), stored(change.after)];
  const exact =
    storedBefore !== undefined &&
    storedAfter !== undefined &&
    JSON.stringify(storedBefore) !== JSON.stringify(storedAfter) &&
    collapsed(words(change.before) ?? "") === collapsed(words(change.after) ?? "");
  const before = exact ? escaped(storedBefore) : words(change.before);
  const after = exact ? escaped(storedAfter) : words(change.after);
  return {
    field: change.field,
    label: FIELD_NAMES[change.field] ?? change.field,
    before,
    after,
    long: (before?.length ?? 0) > LONG || (after?.length ?? 0) > LONG,
    exact,
  };
}

export function changeLines(
  fields: readonly FieldChange[],
  statusName: (key: string) => string,
): readonly ChangeLine[] {
  const lines = fields
    .filter((change) => !BOOKKEEPING.has(change.field))
    .map((change) => changeLine(change, statusName));
  const reordered = fields.some((change) => change.field === "order");
  return lines.length === 0 && reordered
    ? [
        {
          field: "order",
          label: "Order",
          before: undefined,
          after: "moved in the list",
          long: false,
          exact: false,
        },
      ]
    : lines;
}

export const PRIORITY_CHOICES: readonly number[] = [1, 2, 3, 4, 0];

export function labelsOf(text: string): readonly string[] {
  return [
    ...new Set(
      text
        .split(/[\s,]+/u)
        .map((each) => each.trim().toLowerCase())
        .filter((each) => each.length > 0),
    ),
  ];
}
