import { describe, expect, onTestFinished, test } from "vitest";
import { createActor, waitFor } from "xstate";

import type { IssueSummary } from "../board/model.ts";
import { Calls, HANG_GUARD, summary } from "../test/fixtures.ts";
import { canSubmit, createMachine, emptyDraft, newIssue, type Draft } from "./create.machine.ts";

function form() {
  const creations = new Calls<{ draft: Draft; resolve: (issue: IssueSummary) => void }>();
  const created: string[] = [];
  let dismissed = 0;
  const actor = createActor(
    createMachine({
      create: async (draft) =>
        await new Promise<IssueSummary>((resolve) => {
          creations.add({ draft, resolve });
        }),
      parents: async (project) => [summary(`${project}-1`, { project })],
      created: (issue) => created.push(issue.id),
      dismissed: () => {
        dismissed += 1;
      },
    }),
    { input: { project: "DEM" } },
  );
  actor.start();
  onTestFinished(() => {
    actor.stop();
  });
  return { actor, creations, created, dismissed: () => dismissed };
}

describe("the create form", () => {
  test("changing the project clears the status and the parent, and offers the new project's issues as parents", async () => {
    const { actor } = form();
    await waitFor(actor, (snapshot) => snapshot.context.parents.length > 0, HANG_GUARD);
    actor.send({ type: "field", name: "status", value: "doing" });
    actor.send({ type: "field", name: "parent", value: "DEM-1" });

    actor.send({ type: "project", key: "WEB" });
    await waitFor(actor, (snapshot) => snapshot.matches({ parents: "ready" }), HANG_GUARD);

    const { draft, parents } = actor.getSnapshot().context;
    expect(draft).toMatchObject({ project: "WEB", status: "", parent: "" });
    expect(parents.map((issue) => issue.id)).toEqual(["WEB-1"]);
  });

  test("while the issue is created, nothing typed changes the draft and the form cannot be dismissed", async () => {
    const { actor, creations, created, dismissed } = form();
    actor.send({ type: "field", name: "title", value: "Write the guide" });
    actor.send({ type: "field", name: "acceptance", value: "The guide builds." });
    actor.send({ type: "submit" });
    const creation = await creations.nth(1);

    actor.send({ type: "field", name: "title", value: "Something else" });
    actor.send({ type: "dismiss" });
    creation.resolve(summary("DEM-7", { title: "Write the guide" }));
    await waitFor(actor, (snapshot) => snapshot.matches({ form: "created" }), HANG_GUARD);

    expect(creation.draft.title).toBe("Write the guide");
    expect(actor.getSnapshot().context.draft.title).toBe("Write the guide");
    expect(created).toEqual(["DEM-7"]);
    expect(dismissed()).toBe(0);
  });

  test("the form does not submit without acceptance criteria, and sends them trimmed once given", async () => {
    const { actor, creations } = form();
    actor.send({ type: "field", name: "title", value: "Write the guide" });
    actor.send({ type: "submit" });
    actor.send({ type: "field", name: "acceptance", value: " \n\t " });
    actor.send({ type: "submit" });
    const unsent = actor.getSnapshot().matches({ form: "editing" });

    actor.send({ type: "field", name: "acceptance", value: "  The guide builds.  " });
    actor.send({ type: "submit" });
    const creation = await creations.nth(1);

    expect(unsent).toBe(true);
    expect(canSubmit({ ...emptyDraft("DEM"), title: "Write the guide" })).toBe(false);
    expect(newIssue(creation.draft, []).acceptance).toBe("The guide builds.");
  });
});
