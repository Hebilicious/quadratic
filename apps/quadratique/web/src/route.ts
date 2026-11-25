import { shallowRef, type ShallowRef } from "vue";

import catalog from "quadratique-catalog/quadratique.acidcat.json" with { type: "json" };

import { CategorySchema, type Category } from "./board/model.ts";
import {
  ALL,
  CLAIM_FILTERS,
  isGrouping,
  NO_FILTER,
  projectScope,
  type AssigneeFilter,
  type ClaimFilter,
  type Filter,
  type Grouping,
  type Scope,
} from "./board/view.ts";

export interface ViewQuery {
  readonly group: Grouping;
  readonly filter: Filter;
  readonly at: string | undefined;
}

export const NOW_QUERY: ViewQuery = { group: "status", filter: NO_FILTER, at: undefined };

export type Route =
  | { readonly view: "home" }
  | { readonly view: "list"; readonly scope: Scope; readonly query: ViewQuery }
  | { readonly view: "board"; readonly scope: Scope; readonly query: ViewQuery }
  | { readonly view: "history"; readonly scope: Scope }
  | { readonly view: "issue"; readonly issue: string; readonly finishing: boolean }
  | { readonly view: "now"; readonly section: "agents" | "waiting" }
  | { readonly view: "missing" };

const PROJECT = /^\/projects\/([^/]+)(?:\/(board|history))?\/?$/u;
const EVERY_PROJECT = /^\/all(?:\/(board|history))?\/?$/u;
const ISSUE = /^\/issues\/([^/]+)\/?$/u;

const types = catalog.$defs;
const listInput = catalog.capabilities["issue.list"].input.properties;

export const LIMITS = {
  statuses: listInput.status.maxItems,
  categories: listInput.category.maxItems,
  labels: listInput.labels.maxItems,
  text: listInput.text.maxLength,
  status: { maxLength: types.Status.maxLength, pattern: types.Status.pattern },
  label: { maxLength: types.Label.maxLength, pattern: types.Label.pattern },
  role: { maxLength: types.Role.maxLength, pattern: types.Role.pattern },
  time: types.Time.pattern,
  commit: types.CommitHash.pattern,
} as const;

export const firstCharacters = (value: string, count: number): string =>
  [...value].slice(0, count).join("");

const matching = (pattern: string) => {
  const compiled = new RegExp(pattern, "u");
  return (value: string): boolean => compiled.test(value);
};

const fits = (schema: { readonly maxLength: number; readonly pattern: string }) => {
  const matches = matching(schema.pattern);
  return (value: string): boolean => [...value].length <= schema.maxLength && matches(value);
};

const isStatus = fits(LIMITS.status);
const isLabel = fits(LIMITS.label);
const isRole = fits(LIMITS.role);
const isMoment = matching(LIMITS.time);
const isCommit = matching(LIMITS.commit);

const isCategory = (value: string): value is Category =>
  CategorySchema.literals.some((category) => category === value);
const isClaim = (value: string): value is ClaimFilter =>
  CLAIM_FILTERS.some((claim) => claim === value);

const listOf = (search: URLSearchParams, name: string): readonly string[] => [
  ...new Set(
    (search.get(name) ?? "")
      .split(",")
      .map((each) => each.trim())
      .filter((each) => each.length > 0),
  ),
];

function assigneeOf(value: string | null): AssigneeFilter | undefined {
  if (value === "nobody" || value === "human") {
    return { kind: value };
  }
  const role = value?.startsWith("role:") === true ? value.slice("role:".length) : undefined;
  return role !== undefined && isRole(role) ? { kind: "role", role } : undefined;
}

const assigneeText = (assignee: AssigneeFilter): string =>
  assignee.kind === "role" ? `role:${assignee.role}` : assignee.kind;

export function filterOf(search: URLSearchParams): Filter {
  const ready = search.get("ready");
  const claim = search.get("claim") ?? "";
  const text = search.get("text") ?? "";
  return {
    statuses: listOf(search, "status").filter(isStatus).slice(0, LIMITS.statuses),
    categories: listOf(search, "category").filter(isCategory).slice(0, LIMITS.categories),
    priorities: listOf(search, "priority")
      .filter((priority) => /^[0-4]$/u.test(priority))
      .map(Number),
    labels: listOf(search, "label").filter(isLabel).slice(0, LIMITS.labels),
    assignee: assigneeOf(search.get("assignee")),
    claim: isClaim(claim) ? claim : undefined,
    ready: ready === "true" ? true : ready === "false" ? false : undefined,
    text: firstCharacters(text, LIMITS.text),
  };
}

export function filterEntries(filter: Filter): readonly (readonly [string, string])[] {
  const entries: [string, string][] = [];
  const add = (name: string, values: readonly (string | number)[]) => {
    if (values.length > 0) {
      entries.push([name, values.map(String).join(",")]);
    }
  };
  add("status", filter.statuses);
  add("category", filter.categories);
  add("priority", filter.priorities);
  add("label", filter.labels);
  if (filter.assignee !== undefined) {
    entries.push(["assignee", assigneeText(filter.assignee)]);
  }
  if (filter.claim !== undefined) {
    entries.push(["claim", filter.claim]);
  }
  if (filter.ready !== undefined) {
    entries.push(["ready", String(filter.ready)]);
  }
  if (filter.text.trim().length > 0) {
    entries.push(["text", filter.text]);
  }
  return entries;
}

export function queryOf(search: URLSearchParams): ViewQuery {
  const group = search.get("group") ?? "";
  const at = search.get("at") ?? "";
  return {
    group: isGrouping(group) ? group : "status",
    filter: filterOf(search),
    at: isMoment(at) || isCommit(at) ? at : undefined,
  };
}

export function searchOf(query: ViewQuery): string {
  const search = new URLSearchParams();
  if (query.group !== "status") {
    search.set("group", query.group);
  }
  for (const [name, value] of filterEntries(query.filter)) {
    search.set(name, value);
  }
  if (query.at !== undefined) {
    search.set("at", query.at);
  }
  const text = search.toString();
  return text.length === 0 ? "" : `?${text}`;
}

const viewOf = (part: string | undefined) =>
  part === "board" ? "board" : part === "history" ? "history" : "list";

function scopedRoute(scope: Scope, part: string | undefined, search: URLSearchParams): Route {
  const view = viewOf(part);
  return view === "history" ? { view, scope } : { view, scope, query: queryOf(search) };
}

export function routeOf(pathname: string, hash = "", search = ""): Route {
  if (pathname === "/" || pathname === "") {
    return { view: "home" };
  }
  if (pathname === "/now") {
    return { view: "now", section: hash === "#waiting" ? "waiting" : "agents" };
  }
  const params = new URLSearchParams(search);
  const every = EVERY_PROJECT.exec(pathname);
  if (every !== null) {
    return scopedRoute(ALL, every[1], params);
  }
  const project = PROJECT.exec(pathname);
  if (project?.[1] !== undefined) {
    return scopedRoute(projectScope(decodeURIComponent(project[1])), project[2], params);
  }
  const issue = ISSUE.exec(pathname);
  if (issue?.[1] !== undefined) {
    return { view: "issue", issue: decodeURIComponent(issue[1]), finishing: hash === "#finish" };
  }
  return { view: "missing" };
}

const scopePath = (scope: Scope): string =>
  scope.kind === "all" ? "/all" : `/projects/${encodeURIComponent(scope.key)}`;

export const paths = {
  list: (scope: Scope, query: ViewQuery = NOW_QUERY) => `${scopePath(scope)}${searchOf(query)}`,
  board: (scope: Scope, query: ViewQuery = NOW_QUERY) =>
    `${scopePath(scope)}/board${searchOf(query)}`,
  history: (scope: Scope) => `${scopePath(scope)}/history`,
  issue: (issue: string) => `/issues/${encodeURIComponent(issue)}`,
  finishing: (issue: string) => `/issues/${encodeURIComponent(issue)}#finish`,
  now: () => "/now",
  waiting: () => "/now#waiting",
};

export function withQuery(
  route: Extract<Route, { view: "list" | "board" }>,
  query: ViewQuery,
): string {
  return route.view === "list" ? paths.list(route.scope, query) : paths.board(route.scope, query);
}

export function switched(route: Route): string | undefined {
  if (route.view === "list") {
    return paths.board(route.scope, route.query);
  }
  return route.view === "board" ? paths.list(route.scope, route.query) : undefined;
}

export const atPast = (route: Route): boolean =>
  (route.view === "list" || route.view === "board") && route.query.at !== undefined;

export function projectOfIssue(issue: string): string {
  return issue.slice(0, issue.lastIndexOf("-"));
}

export function createRouter(view: Window): {
  readonly route: ShallowRef<Route>;
  readonly go: (path: string) => void;
  readonly replace: (path: string) => void;
} {
  const read = () => routeOf(view.location.pathname, view.location.hash, view.location.search);
  const route = shallowRef(read());
  const here = () => view.location.pathname + view.location.search + view.location.hash;
  view.addEventListener("popstate", () => {
    route.value = read();
  });
  return {
    route,
    go: (path) => {
      if (path !== here()) {
        view.history.pushState(null, "", path);
      }
      route.value = read();
    },
    replace: (path) => {
      if (path !== here()) {
        view.history.replaceState(null, "", path);
      }
      route.value = read();
    },
  };
}
