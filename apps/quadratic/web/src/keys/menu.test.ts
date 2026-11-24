import { describe, expect, test } from "vitest";

import type { IssueSummary } from "../board/model.ts";
import { summary, demo } from "../test/fixtures.ts";
import { ALL, projectScope } from "../board/view.ts";
import { NOW_QUERY } from "../route.ts";
import { matching, menuItems, type MenuItem } from "./menu.ts";

const offered = (targets: readonly IssueSummary[], query = ""): readonly MenuItem[] =>
  menuItems({
    projects: [demo],
    view: "list",
    route: { view: "list", scope: projectScope("DEM"), query: NOW_QUERY },
    targets,
    roles: [],
    query,
  });

const action = (items: readonly MenuItem[], id: string) =>
  items.find((item) => item.id === id)?.action;

describe("the command menu", () => {
  test("it moves the selection to any status that not every selected issue is in, and assigns it to a role typed after Assign", () => {
    const items = offered(
      [summary("DEM-1"), summary("DEM-2", { status: "doing" })],
      "assign web-design",
    );
    expect(action(items, "status:doing")).toEqual({
      kind: "run",
      commands: [
        { kind: "update", issue: "DEM-1", revision: 1, patch: { status: "doing" } },
        { kind: "update", issue: "DEM-2", revision: 1, patch: { status: "doing" } },
      ],
    });
    expect(action(offered([summary("DEM-1")]), "status:todo")).toBeUndefined();
    expect(matching(items, "assign web-design").map((item) => item.action)).toEqual([
      {
        kind: "run",
        commands: [
          {
            kind: "update",
            issue: "DEM-1",
            revision: 1,
            patch: { assignee: { kind: "agent", role: "web-design" } },
          },
          {
            kind: "update",
            issue: "DEM-2",
            revision: 1,
            patch: { assignee: { kind: "agent", role: "web-design" } },
          },
        ],
      },
    ]);
  });

  test("it peeks at the issue it names", () => {
    expect(action(offered([summary("DEM-2")]), "peek")).toEqual({ kind: "peek", issue: "DEM-2" });
  });

  test("it offers Close only where closing changes the issue: not in the workflow's merge status, not when resolved", () => {
    expect(
      action(offered([summary("DEM-1", { status: "checking", category: "started" })]), "close"),
    ).toEqual({
      kind: "go",
      path: "/issues/DEM-1#close",
    });
    expect(
      action(offered([summary("DEM-1", { status: "landed", category: "started" })]), "close"),
    ).toBeUndefined();
    expect(
      action(offered([summary("DEM-1", { status: "done", category: "completed" })]), "close"),
    ).toBeUndefined();
  });

  test("on every project's list, it offers the statuses and Close of the issue's own project", () => {
    const items = menuItems({
      projects: [demo],
      view: "list",
      route: { view: "list", scope: ALL, query: NOW_QUERY },
      targets: [summary("DEM-1", { status: "checking", category: "started" })],
      roles: [],
      query: "",
    });
    expect([action(items, "status:doing"), action(items, "close")]).toEqual([
      {
        kind: "run",
        commands: [{ kind: "update", issue: "DEM-1", revision: 1, patch: { status: "doing" } }],
      },
      { kind: "go", path: "/issues/DEM-1#close" },
    ]);
  });
});
