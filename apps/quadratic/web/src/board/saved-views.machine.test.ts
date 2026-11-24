import { describe, expect, test } from "vitest";
import { createActor } from "xstate";

import { refusedWrite, savedViewOf, savedViewsMachine } from "./saved-views.machine.ts";
import { ALL, NO_FILTER, projectScope } from "./view.ts";

function browser(initial: unknown = undefined) {
  let stored = initial;
  return {
    ports: {
      read: () => stored,
      write: (next: unknown) => {
        stored = JSON.parse(JSON.stringify(next));
      },
    },
    open() {
      const actor = createActor(savedViewsMachine(this.ports));
      actor.start();
      return actor;
    },
  };
}

const ready = { ...NO_FILTER, labels: ["web"], ready: true };

describe("saved views", () => {
  test("a view saved, renamed and deleted is what a later page of the same browser finds", () => {
    const storage = browser();
    const first = storage.open();
    first.send({
      type: "save",
      form: "t",
      name: "Ready web",
      scope: projectScope("DEM"),
      filter: ready,
    });
    first.send({
      type: "save",
      form: "t",
      name: "Mine",
      scope: ALL,
      filter: { ...NO_FILTER, claim: "mine" },
    });
    const [web, mine] = first.getSnapshot().context.views;
    first.send({ type: "rename", form: "t", id: web?.id ?? "", name: "Web, ready" });
    first.send({ type: "remove", form: "t", id: mine?.id ?? "" });
    first.stop();

    expect(storage.open().getSnapshot().context.views).toEqual([
      { id: "v1", name: "Web, ready", scope: projectScope("DEM"), filter: ready },
    ]);
  });

  test("saving under a name already taken replaces that view's filters; renaming to a taken or empty name changes nothing", () => {
    const actor = browser().open();
    actor.send({ type: "save", form: "t", name: "Web", scope: ALL, filter: NO_FILTER });
    actor.send({ type: "save", form: "t", name: "Other", scope: ALL, filter: NO_FILTER });
    actor.send({
      type: "save",
      form: "t",
      name: " web ",
      scope: projectScope("DEM"),
      filter: ready,
    });
    actor.send({ type: "rename", form: "t", id: "v2", name: "WEB" });
    actor.send({ type: "rename", form: "t", id: "v2", name: "  " });
    expect(
      actor.getSnapshot().context.views.map((view) => [view.name, view.scope, view.filter]),
    ).toEqual([
      ["Web", projectScope("DEM"), ready],
      ["Other", ALL, NO_FILTER],
    ]);
  });

  test("what the browser stored in another shape is no saved view, not a failure", () => {
    expect(browser({ version: 9, views: "nope" }).open().getSnapshot().context.views).toEqual([]);
  });

  test("on the filters of a saved view in another order, the page finds that view", () => {
    const views = [
      {
        id: "v1",
        name: "Docs",
        scope: ALL,
        filter: { ...NO_FILTER, labels: ["web", "docs"], priorities: [2, 1] },
      },
    ];
    expect(
      savedViewOf(views, ALL, { ...NO_FILTER, labels: ["docs", "web"], priorities: [1, 2] })?.id,
    ).toBe("v1");
  });

  describe("when the browser refuses to store a change", () => {
    function refusing() {
      const storage = { refuses: false, stored: undefined as unknown };
      const actor = createActor(
        savedViewsMachine({
          read: () => storage.stored,
          write: (next) => {
            if (storage.refuses) {
              throw new DOMException("The quota has been exceeded.", "QuotaExceededError");
            }
            storage.stored = next;
          },
        }),
      );
      actor.start();
      actor.send({ type: "save", form: "first", name: "Ready web", scope: ALL, filter: ready });
      storage.refuses = true;
      return {
        actor,
        storage,
        refused: (form: string) => refusedWrite(actor.getSnapshot(), form),
      };
    }
    const names = (actor: ReturnType<typeof refusing>["actor"]) =>
      actor.getSnapshot().context.views.map((view) => view.name);

    test("a save is refused and shows no new view", () => {
      const { actor, refused } = refusing();
      actor.send({ type: "save", form: "save", name: "Mine", scope: ALL, filter: NO_FILTER });
      expect([refused("save"), names(actor)]).toEqual([true, ["Ready web"]]);
    });

    test("a rename is refused and keeps the name", () => {
      const { actor, refused } = refusing();
      actor.send({ type: "rename", form: "rename", id: "v1", name: "Web, ready" });
      expect([refused("rename"), names(actor)]).toEqual([true, ["Ready web"]]);
    });

    test("a delete is refused and keeps the view", () => {
      const { actor, refused } = refusing();
      actor.send({ type: "remove", form: "remove", id: "v1" });
      expect([refused("remove"), names(actor)]).toEqual([true, ["Ready web"]]);
    });

    test("a refused rename stays refused when another form's save is stored", () => {
      const { actor, storage, refused } = refusing();
      actor.send({ type: "rename", form: "rename", id: "v1", name: "Web, ready" });
      storage.refuses = false;
      actor.send({ type: "save", form: "save", name: "Mine", scope: ALL, filter: NO_FILTER });
      expect([refused("rename"), refused("save")]).toEqual([true, false]);
    });

    test("retries of one form keep one refusal, and the form's leaving forgets it", () => {
      const { actor } = refusing();
      for (let attempt = 0; attempt < 3; attempt += 1) {
        actor.send({ type: "save", form: "save", name: "Mine", scope: ALL, filter: NO_FILTER });
      }
      const afterRetries = actor.getSnapshot().context.refused;
      actor.send({ type: "forget", form: "save" });
      expect([afterRetries, actor.getSnapshot().context.refused]).toEqual([["save"], []]);
    });
  });
});
