import { describe, expect, test } from "vitest";

import { summary, demo } from "../test/fixtures.ts";
import { issuesById } from "./issues.ts";
import type { Project } from "./model.ts";
import {
  ALL,
  columnsOf,
  grouped,
  matches,
  NO_FILTER,
  projectScope,
  shiftPatch,
  shownGroups,
  type GroupingContext,
} from "./view.ts";

const quadratic: Project = {
  key: "QUA",
  name: "Quadratic",
  workflow: {
    statuses: [
      { key: "todo", name: "To do", category: "unstarted" },
      { key: "review", name: "Review", category: "started" },
      { key: "idea", name: "Idea", category: "backlog" },
      { key: "done", name: "Done", category: "completed" },
    ],
    moves: { claim: "review", review: "review", merge: "done", release: "todo" },
  },
};

const context = (fields: Partial<GroupingContext> = {}): GroupingContext => ({
  projects: [demo, quadratic],
  scope: projectScope("DEM"),
  unfiltered: {},
  milestones: {},
  ...fields,
});

const names = (groups: ReturnType<typeof grouped>) =>
  groups.map((group) => [group.name, group.issues.map((issue) => issue.id)]);

describe("grouping a list or a board", () => {
  test("by label, an issue with two labels is in both groups, the unlabelled ones have their own, and each group keeps the list's order", () => {
    const issues = [
      summary("DEM-3", { labels: ["web"], order: 3 }),
      summary("DEM-1", { labels: ["web", "server"], order: 1 }),
      summary("DEM-2", { labels: [], order: 2 }),
    ];
    expect(names(grouped("label", issues, context()))).toEqual([
      ["server", ["DEM-1"]],
      ["web", ["DEM-1", "DEM-3"]],
      ["No label", ["DEM-2"]],
    ]);
  });

  test("the cursor stops at an issue in several groups only where it first appears", () => {
    const issues = [
      summary("DEM-1", { labels: ["server", "web"], order: 1 }),
      summary("DEM-2", { labels: ["web"], order: 2 }),
    ];
    expect(columnsOf(grouped("label", issues, context()))).toEqual([["DEM-1"], ["DEM-2"]]);
  });

  test("by priority, every priority is a group from urgent to none, empty or not", () => {
    const issues = [summary("DEM-1", { priority: 0 }), summary("DEM-2", { priority: 2 })];
    expect(names(grouped("priority", issues, context()))).toEqual([
      ["Urgent", []],
      ["High", ["DEM-2"]],
      ["Medium", []],
      ["Low", []],
      ["No priority", ["DEM-1"]],
    ]);
  });

  test("by status across projects, each status key is one group, in the order of the categories", () => {
    const groups = grouped("status", [], context({ scope: ALL }));
    expect(groups.map((group) => group.key)).toEqual([
      "backlog",
      "idea",
      "todo",
      "doing",
      "checking",
      "landed",
      "review",
      "done",
    ]);
  });

  test("by status, an issue whose status the workflow no longer has is in a group named by that status, not dropped", () => {
    const issues = [summary("DEM-1", { status: "triage", category: "backlog" })];
    const groups = grouped("status", issues, context());
    expect(groups.find((group) => group.name === "triage")?.issues.map((each) => each.id)).toEqual([
      "DEM-1",
    ]);
  });

  test("by parent, a parent's group is titled by the parent, issues with none have their own group, kept on the board when empty", () => {
    const parent = summary("DEM-1", { title: "Ship the board", order: 1 });
    const child = summary("DEM-2", { parent: "DEM-1", order: 2 });
    const groups = grouped("parent", [child], context({ unfiltered: issuesById([parent, child]) }));
    expect(names(groups)).toEqual([
      ["DEM-1 Ship the board", ["DEM-2"]],
      ["No parent", []],
    ]);
  });

  test("by milestone, from the milestones read for each issue", () => {
    const issues = [summary("DEM-1"), summary("DEM-2"), summary("DEM-3")];
    const groups = grouped(
      "milestone",
      issues,
      context({ milestones: { "DEM-1": "m3", "DEM-2": null, "DEM-3": "m3" } }),
    );
    expect(names(groups)).toEqual([
      ["m3", ["DEM-1", "DEM-3"]],
      ["No milestone", ["DEM-2"]],
    ]);
  });

  test("by project, one group for each project that has issues", () => {
    const issues = [summary("QUA-1", { project: "QUA" }), summary("DEM-1")];
    expect(names(grouped("project", issues, context({ scope: ALL })))).toEqual([
      ["Demo", ["DEM-1"]],
      ["Quadratic", ["QUA-1"]],
    ]);
  });
});

describe("moving an issue to the next group", () => {
  const groupsOf = (issues: ReturnType<typeof summary>[]) => grouped("priority", issues, context());

  test("by priority, Shift L moves an urgent issue to high, and Shift H at the first group does nothing", () => {
    const issue = summary("DEM-1", { priority: 1 });
    const groups = groupsOf([issue]);
    const ports = { projects: [demo], milestones: {} };
    expect(shiftPatch("priority", groups, issue, 1, ports)).toEqual({ priority: 2 });
    expect(shiftPatch("priority", groups, issue, -1, ports)).toBeUndefined();
  });

  test("by label or project there is no single value to change, so nothing moves", () => {
    const issue = summary("DEM-1", { labels: ["web"] });
    const groups = grouped("label", [issue], context());
    expect(
      shiftPatch("label", groups, issue, 1, { projects: [demo], milestones: {} }),
    ).toBeUndefined();
  });

  test("by parent, moving to the group of no parent clears the parent", () => {
    const parent = summary("DEM-1", { order: 1 });
    const child = summary("DEM-2", { parent: "DEM-1", order: 2 });
    const groups = grouped("parent", [child], context({ unfiltered: issuesById([parent, child]) }));
    expect(shiftPatch("parent", groups, child, 1, { projects: [demo], milestones: {} })).toEqual({
      clear: ["parent"],
    });
  });
});

describe("filtering a list or a board", () => {
  test("an issue passes only when it passes every member; statuses and categories are one member", () => {
    const filter = {
      ...NO_FILTER,
      statuses: ["doing"],
      categories: ["backlog" as const],
      labels: ["web", "server"],
      text: "board",
    };
    const passing = summary("DEM-1", {
      status: "backlog",
      category: "backlog",
      labels: ["server", "web", "ui"],
      title: "Ship the Board",
    });
    expect(matches(filter, passing)).toBe(true);
    expect(matches(filter, { ...passing, labels: ["web"] })).toBe(false);
    expect(matches(filter, { ...passing, status: "todo", category: "unstarted" })).toBe(false);
    expect(matches(filter, { ...passing, title: "Ship the list" })).toBe(false);
  });

  test("assignee, claim and ready each keep only the issues that answer them", () => {
    const claim = {
      holder: { kind: "agent" as const, name: "codex@dem-2" },
      branch: "dem-2",
      worktree: "/w/dem-2",
      since: "2026-11-19T10:00:00Z",
    };
    const issues = issuesById([
      summary("DEM-1", { assignee: { kind: "agent", role: "security" } }),
      summary("DEM-2", { claim, ready: false }),
      summary("DEM-3", { assignee: { kind: "human" } }),
    ]);
    const shown = (filter: typeof NO_FILTER) =>
      shownGroups({
        grouping: "status",
        filter,
        scope: projectScope("DEM"),
        projects: [demo],
        issues,
        milestones: {},
      }).flatMap((group) => group.issues.map((issue) => issue.id));
    expect(shown({ ...NO_FILTER, assignee: { kind: "role", role: "security" } })).toEqual([
      "DEM-1",
    ]);
    expect(shown({ ...NO_FILTER, assignee: { kind: "nobody" } })).toEqual(["DEM-2"]);
    expect(shown({ ...NO_FILTER, claim: "any" })).toEqual(["DEM-2"]);
    expect(shown({ ...NO_FILTER, claim: "none", ready: true })).toEqual(["DEM-1", "DEM-3"]);
  });
});
