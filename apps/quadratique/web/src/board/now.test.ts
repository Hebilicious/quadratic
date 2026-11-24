import { describe, expect, test } from "vitest";

import { summary, demo } from "../test/fixtures.ts";
import { issuesById } from "./issues.ts";
import { appliedToNow, nowView } from "./now.ts";

const claim = (name: string, since: string) => ({
  holder: { kind: "agent" as const, name },
  branch: `${name}-branch`,
  worktree: `/work/${name}`,
  since,
});

describe("the now view", () => {
  test("it groups every claim by its holder, a name in one worktree, and what waits on the human is read from the workflow, not from status names", () => {
    const now = issuesById([
      summary("DEM-1", {
        status: "doing",
        category: "started",
        claim: claim("a@one", "2026-11-18T10:00:00Z"),
      }),
      summary("DEM-2", {
        status: "checking",
        category: "started",
        claim: claim("a@one", "2026-11-18T09:00:00Z"),
      }),
      summary("DEM-3", {
        status: "doing",
        category: "started",
        claim: claim("b@two", "2026-11-18T11:00:00Z"),
      }),
      summary("DEM-4", { assignee: { kind: "human" }, priority: 4 }),
      summary("DEM-5", { assignee: { kind: "human" }, priority: 1 }),
      summary("DEM-6", { assignee: { kind: "human" }, status: "done", category: "completed" }),
      summary("DEM-7", { status: "landed", category: "started" }),
      summary("DEM-8", {
        status: "doing",
        category: "started",
        claim: { ...claim("a@one", "2026-11-18T12:00:00Z"), worktree: "/work/elsewhere" },
      }),
    ]);
    const view = nowView(now, [demo]);
    expect(
      view.agents.map((agent) => [
        agent.holder.name,
        agent.worktree,
        agent.issues.map((each) => each.id),
      ]),
    ).toEqual([
      ["a@one", "/work/a@one", ["DEM-2", "DEM-1"]],
      ["b@two", "/work/b@two", ["DEM-3"]],
      ["a@one", "/work/elsewhere", ["DEM-8"]],
    ]);
    expect(view.assigned.map((each) => each.id)).toEqual(["DEM-5", "DEM-4"]);
    expect(view.review.map((each) => each.id)).toEqual(["DEM-2"]);
  });

  test("an issue that left the view does not come back with an older answer that arrives after it left", () => {
    const assigned = summary("DEM-1", { assignee: { kind: "human" } });
    const before = issuesById([assigned]);
    const left = appliedToNow(
      before,
      {
        from: "change",
        at: "2026-11-18T10:00:03.000Z",
        issues: [{ ...assigned, status: "done", category: "completed", revision: 3 }],
      },
      undefined,
    );
    const late = appliedToNow(
      left,
      { from: "answer", issues: [{ ...assigned, priority: 1, revision: 2 }] },
      undefined,
    );
    expect(nowView(late, [demo]).assigned).toEqual([]);
  });
});
