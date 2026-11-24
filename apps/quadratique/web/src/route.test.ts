import { describe, expect, test } from "vitest";

import { NO_FILTER, projectScope } from "./board/view.ts";
import { LIMITS, paths, queryOf, routeOf, searchOf, type ViewQuery } from "./route.ts";

describe("the address of a list or a board", () => {
  test("its grouping, filters and moment survive a round trip through the address", () => {
    const query: ViewQuery = {
      group: "label",
      filter: {
        ...NO_FILTER,
        categories: ["started"],
        priorities: [1, 2],
        labels: ["web", "server"],
        assignee: { kind: "role", role: "security" },
        claim: "none",
        ready: true,
        text: "the board",
      },
      at: "2026-11-19T09:30:00Z",
    };
    const path = paths.board(projectScope("DEM"), query);
    const url = new URL(path, "http://127.0.0.1");
    expect(routeOf(url.pathname, url.hash, url.search)).toEqual({
      view: "board",
      scope: projectScope("DEM"),
      query,
    });
  });

  test("a value the catalog would refuse is left out, and the defaults leave the address bare", () => {
    expect(
      queryOf(new URLSearchParams("group=colour&priority=9&label=Bad Label&at=yesterday")),
    ).toEqual({
      group: "status",
      filter: NO_FILTER,
      at: undefined,
    });
    expect(searchOf({ group: "status", filter: NO_FILTER, at: undefined })).toBe("");
  });

  test("a moment that never was and values longer than the catalog takes are left out", () => {
    const long = (length: number) => "a".repeat(length);
    const search = new URLSearchParams({
      at: "2026-02-30T12:00:00Z",
      status: `${long(33)},todo`,
      label: `${long(49)},web`,
      assignee: `role:${long(33)}`,
    });
    expect(queryOf(search)).toEqual({
      group: "status",
      filter: { ...NO_FILTER, statuses: ["todo"], labels: ["web"] },
      at: undefined,
    });
    expect(queryOf(new URLSearchParams("at=2026-13-01T12:00:00Z")).at).toBeUndefined();
    expect(queryOf(new URLSearchParams("at=2028-02-29T23:59:59Z")).at).toBe("2028-02-29T23:59:59Z");
  });

  test("a list longer than the catalog takes keeps its first values, and text keeps its first characters, counted as the server counts", () => {
    const many = (count: number, name: string) =>
      Array.from({ length: count }, (_, index) => `${name}${String(index)}`);
    const filter = queryOf(
      new URLSearchParams({
        status: many(21, "s").join(","),
        label: many(11, "l").join(","),
        text: "𝒜".repeat(201),
      }),
    ).filter;
    expect([filter.statuses, filter.labels, filter.text]).toEqual([
      many(20, "s"),
      many(10, "l"),
      "𝒜".repeat(200),
    ]);
  });

  test("the catalog carries every limit and pattern the address reads", () => {
    const { statuses, categories, labels, text, status, label, role, time, commit } = LIMITS;
    for (const limit of [
      statuses,
      categories,
      labels,
      text,
      status.maxLength,
      label.maxLength,
      role.maxLength,
    ]) {
      expect(Number.isInteger(limit) && limit > 0, String(limit)).toBe(true);
    }
    for (const pattern of [status.pattern, label.pattern, role.pattern, time, commit]) {
      expect(typeof pattern === "string" && pattern.length > 0, String(pattern)).toBe(true);
    }
  });

  test("every project has its own list, board and history", () => {
    expect(routeOf("/all/history")).toEqual({ view: "history", scope: { kind: "all" } });
    expect(routeOf("/all")).toMatchObject({ view: "list", scope: { kind: "all" } });
  });
});
