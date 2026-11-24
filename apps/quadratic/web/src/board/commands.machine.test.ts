import { describe, expect, test } from "vitest";
import { createActor, waitFor } from "xstate";

import type { IssuePatch } from "../core/quadratic.ts";
import { Refused } from "../core/quadratic.ts";
import { Calls, HANG_GUARD, summary, demo } from "../test/fixtures.ts";
import { commandsMachine, type Command } from "./commands.machine.ts";
import { grouped, projectScope, shiftPlan, type Grouping } from "./view.ts";
import type { IssueSummary } from "./model.ts";

function server(issues: readonly IssueSummary[]) {
  const board: Record<string, IssueSummary> = Object.fromEntries(
    issues.map((issue) => [issue.id, issue]),
  );
  const sent: { issue: string; patch: IssuePatch }[] = [];
  const held = new Calls<() => void>();
  const holding = new Set<string>();
  const actor = createActor(
    commandsMachine({
      quadratic: {
        update: async ({ issue, revision, patch }) => {
          sent.push({ issue, patch });
          if (holding.has(issue)) {
            holding.delete(issue);
            await new Promise<void>((resolve) => {
              held.add(resolve);
            });
          }
          const current = board[issue];
          if (current === undefined || current.revision !== revision) {
            throw new Refused("issue.update", {
              _tag: "Stale",
              revision: current?.revision ?? 0,
            });
          }
          const status = demo.workflow.statuses.find((each) => each.key === patch.status);
          return {
            ...current,
            ...(patch.priority === undefined ? {} : { priority: patch.priority }),
            status: status?.key ?? current.status,
            category: status?.category ?? current.category,
            revision: revision + 1,
          };
        },
        release: async () => {
          throw new Error("No release in this test");
        },
      },
      current: (issue) => board[issue],
      settle: (settled) => {
        for (const issue of settled) {
          board[issue.id] = issue;
        }
      },
    }),
  );
  actor.start();
  const shift = (id: string, by: -1 | 1, grouping: Grouping): Command => {
    const issue = board[id] ?? summary(id);
    const groups = grouped(grouping, Object.values(board), {
      projects: [demo],
      scope: projectScope("DEM"),
      unfiltered: board,
      milestones: {},
    });
    return {
      kind: "shift",
      issue: id,
      by,
      plan: shiftPlan(grouping, groups, issue, { projects: [demo], milestones: {} }),
    };
  };
  return { actor, board, sent, holding, shift, held: async () => await held.nth(1) };
}

describe("commands from the list and the board", () => {
  test("two moves given quickly in a row both land: neither is refused as stale", async () => {
    const { actor, board, shift } = server([summary("DEM-1")]);
    actor.send({ type: "run", commands: [shift("DEM-1", 1, "status")] });
    actor.send({ type: "run", commands: [shift("DEM-1", 1, "status")] });
    await waitFor(
      actor,
      (snapshot) => snapshot.matches("idle") || snapshot.matches("reported"),
      HANG_GUARD,
    );
    expect(actor.getSnapshot().context.failed).toEqual([]);
    expect(board["DEM-1"]?.status).toBe("checking");
    actor.stop();
  });

  test("a move counts a write still being sent only while the board shows the revision that write was made for", async () => {
    const { actor, board, sent, holding, shift, held } = server([
      summary("DEM-1", { priority: 3 }),
    ]);
    holding.add("DEM-1");
    actor.send({
      type: "run",
      commands: [{ kind: "update", issue: "DEM-1", revision: 1, patch: { priority: 2 } }],
    });
    const release = await held();
    board["DEM-1"] = { ...summary("DEM-1"), priority: 4, revision: 3 };
    actor.send({ type: "run", commands: [shift("DEM-1", -1, "priority")] });
    release();
    await waitFor(
      actor,
      (snapshot) => snapshot.matches("idle") || snapshot.matches("reported"),
      HANG_GUARD,
    );
    expect(sent.at(-1)).toEqual({ issue: "DEM-1", patch: { priority: 3 } });
    actor.stop();
  });

  test("when the page enters the past, the write being sent may finish, and the ones not sent yet are dropped", async () => {
    const { actor, sent, holding, held } = server([summary("DEM-1"), summary("DEM-2")]);
    holding.add("DEM-1");
    actor.send({
      type: "run",
      commands: [
        { kind: "update", issue: "DEM-1", revision: 1, patch: { priority: 1 } },
        { kind: "update", issue: "DEM-2", revision: 1, patch: { priority: 1 } },
      ],
    });
    const release = await held();
    actor.send({ type: "past" });
    release();
    await waitFor(actor, (snapshot) => snapshot.matches("idle"), HANG_GUARD);
    expect(sent.map((each) => each.issue)).toEqual(["DEM-1"]);
    actor.stop();
  });
});
