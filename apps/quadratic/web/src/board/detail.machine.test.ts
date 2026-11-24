import { describe, expect, onTestFinished, test } from "vitest";
import { createActor, waitFor } from "xstate";

import type { Quadratic } from "../core/quadratic.ts";
import { Calls, HANG_GUARD, summary } from "../test/fixtures.ts";
import { detailMachine } from "./detail.machine.ts";
import type { Issue } from "./model.ts";

const issue = (title: string): Issue => ({
  ...summary("DEM-1", { title }),
  notes: [],
  relations: [],
  children: [],
  createdAt: "2026-11-18T10:00:00.000Z",
  createdBy: { kind: "human", name: "tester" },
  updatedAt: "2026-11-18T10:00:00.000Z",
  links: [],
});

function detail() {
  const reads = new Calls<{ resolve: (issue: Issue) => void; reject: (error: unknown) => void }>();
  const quadratic: Pick<Quadratic, "issue" | "history"> = {
    issue: async () =>
      await new Promise<Issue>((resolve, reject) => {
        reads.add({ resolve, reject });
      }),
    history: async () => ({ items: [] }),
  };
  const actor = createActor(detailMachine(quadratic), { input: { id: "DEM-1" } });
  actor.start();
  onTestFinished(() => {
    actor.stop();
  });
  return { actor, read: async (count: number) => await reads.nth(count) };
}

describe("an open issue", () => {
  test("a change that arrives while the issue loads is not lost: the page shows the issue as it is after the change", async () => {
    const { actor, read } = detail();
    const first = await read(1);
    actor.send({ type: "changed", issues: [summary("DEM-1", { title: "After", revision: 2 })] });
    first.resolve(issue("Before"));
    (await read(2)).resolve(issue("After"));

    await waitFor(actor, (snapshot) => snapshot.context.issue?.title === "After", HANG_GUARD);
  });

  test("a read again that fails says that the issue may be out of date, and a retry reads it", async () => {
    const { actor, read } = detail();
    (await read(1)).resolve(issue("Before"));
    await waitFor(actor, (snapshot) => snapshot.context.issue !== undefined, HANG_GUARD);
    actor.send({ type: "reload" });
    (await read(2)).reject(new Error("The server does not answer"));
    await waitFor(
      actor,
      (snapshot) => snapshot.matches({ issue: { ready: "outdated" } }),
      HANG_GUARD,
    );

    actor.send({ type: "retry" });
    (await read(3)).resolve(issue("After"));
    await waitFor(actor, (snapshot) => snapshot.context.issue?.title === "After", HANG_GUARD);
    expect(actor.getSnapshot().matches({ issue: { ready: "current" } })).toBe(true);
  });
});
