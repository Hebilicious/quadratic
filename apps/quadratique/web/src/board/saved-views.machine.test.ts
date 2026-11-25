import { describe, expect, test } from "vitest";
import { createActor, waitFor } from "xstate";

import {
  legacyViews,
  savedViewOf,
  savedViewsMachine,
  writeState,
  type NewView,
  type StoredView,
  type Views,
} from "./saved-views.machine.ts";
import { ALL, NO_FILTER, projectScope } from "./view.ts";

/** A server that keeps views as `views.import` and `view.save` promise, and a live channel for each page. */
function server(initial: Views = []) {
  let views: StoredView[] = [...initial];
  let next = initial.length + 1;
  let refuses = false;
  const channels = new Set<(views: Views) => void>();
  const sameName = (left: string, right: string) =>
    left.trim().toLowerCase() === right.trim().toLowerCase();
  const changed = (): Views => {
    const answer = [...views];
    for (const channel of channels) {
      channel(answer);
    }
    return answer;
  };
  const write = async (change: () => void): Promise<Views> => {
    await Promise.resolve();
    if (refuses) {
      throw new Error("the server did not answer");
    }
    change();
    return changed();
  };
  const add = (view: NewView) => {
    views.push({ id: `v${String(next)}`, ...view });
    next += 1;
  };
  return {
    get views() {
      return views;
    },
    refuse(value: boolean) {
      refuses = value;
    },
    page(legacy: { stored: unknown } = { stored: null }) {
      const actor = createActor(
        savedViewsMachine({
          list: async () => {
            await Promise.resolve();
            return [...views];
          },
          save: (view) =>
            write(() => {
              const named = views.find((each) => sameName(each.name, view.name));
              if (named === undefined) {
                add(view);
              } else {
                views = views.map((each) =>
                  each === named ? { ...view, id: each.id, name: each.name } : each,
                );
              }
            }),
          rename: (id, name) =>
            write(() => {
              views = views.map((each) => (each.id === id ? { ...each, name } : each));
            }),
          remove: (id) =>
            write(() => {
              views = views.filter((each) => each.id !== id);
            }),
          import: (imported) =>
            write(() => {
              for (const view of imported) {
                if (!views.some((each) => sameName(each.name, view.name))) {
                  add(view);
                }
              }
            }),
          changes: async (signal) => {
            await Promise.resolve();
            if (refuses) {
              throw new Error("the server did not answer");
            }
            const queue: Views[] = [];
            let wake: (() => void) | undefined;
            const channel = (views: Views) => {
              queue.push(views);
              wake?.();
            };
            channels.add(channel);
            signal.addEventListener("abort", () => {
              channels.delete(channel);
              wake?.();
            });
            return (async function* () {
              while (!signal.aborted) {
                const views = queue.shift();
                if (views === undefined) {
                  await new Promise<void>((resolve) => {
                    wake = resolve;
                  });
                } else {
                  yield views;
                }
              }
            })();
          },
          legacy: {
            read: () => legacy.stored,
            clear: () => {
              legacy.stored = null;
            },
          },
        }),
      );
      actor.start();
      return actor;
    },
  };
}

type Page = ReturnType<ReturnType<typeof server>["page"]>;

const live = (page: Page) => waitFor(page, (snapshot) => snapshot.context.live);
const names = (page: Page) => page.getSnapshot().context.views.map((view) => view.name);
const settled = (page: Page, form: string) =>
  waitFor(page, (snapshot) => writeState(snapshot, form) !== "pending");

const ready = { ...NO_FILTER, labels: ["web"], ready: true };

describe("saved views", () => {
  test("a view saved, renamed and deleted in one page is what another page of the same server shows", async () => {
    const shared = server();
    const first = shared.page();
    const second = shared.page();
    await Promise.all([live(first), live(second)]);

    first.send({
      type: "save",
      form: "s",
      name: " Ready web ",
      scope: projectScope("DEM"),
      filter: ready,
    });
    await settled(first, "s");
    first.send({
      type: "save",
      form: "s",
      name: "Mine",
      scope: ALL,
      filter: { ...NO_FILTER, claim: "mine" },
    });
    await settled(first, "s");
    first.send({ type: "rename", form: "r", id: "v1", name: "Web, ready" });
    await settled(first, "r");
    first.send({ type: "remove", form: "d", id: "v2" });
    await settled(first, "d");

    await waitFor(second, (snapshot) => snapshot.context.views.length === 1);
    expect(second.getSnapshot().context.views).toEqual([
      { id: "v1", name: "Web, ready", scope: projectScope("DEM"), filter: ready },
    ]);
    expect(shared.views).toEqual([
      { id: "v1", name: "Web, ready", project: "DEM", filter: "label=web&ready=true" },
    ]);
  });

  test("renaming to a name another view has, or to an empty one, sends nothing", async () => {
    const shared = server([
      { id: "v1", name: "Web", filter: "" },
      { id: "v2", name: "Other", filter: "" },
    ]);
    const page = shared.page();
    await waitFor(page, (snapshot) => snapshot.context.views.length === 2);
    page.send({ type: "rename", form: "r", id: "v2", name: "WEB" });
    page.send({ type: "rename", form: "r", id: "v2", name: "  " });
    expect(writeState(page.getSnapshot(), "r")).toBe("idle");
    expect(names(page)).toEqual(["Web", "Other"]);
  });

  test("the views this browser kept are imported once and then forgotten by the browser", async () => {
    const shared = server([{ id: "v1", name: "On the server", filter: "" }]);
    const legacy = {
      stored: {
        version: 1,
        views: [
          { id: "v1", name: "Ready web", project: "DEM", filter: "label=web&ready=true" },
          { id: "v2", name: "Mine", filter: "claim=mine" },
        ],
      },
    };
    const page = shared.page(legacy);
    await waitFor(page, (snapshot) => snapshot.context.views.length === 3);
    expect(names(page)).toEqual(["On the server", "Ready web", "Mine"]);
    expect(legacy.stored).toBeNull();

    const later = shared.page(legacy);
    await live(later);
    await waitFor(later, (snapshot) => snapshot.context.views.length === 3);
    expect(shared.views).toHaveLength(3);
  });

  test("when the import fails, the browser keeps its views for the next page", async () => {
    const shared = server();
    const legacy = {
      stored: { version: 1, views: [{ id: "v1", name: "Mine", filter: "claim=mine" }] },
    };
    shared.refuse(true);
    const page = shared.page(legacy);
    await waitFor(page, (snapshot) => snapshot.matches("reconnecting"));
    expect(legacy.stored).not.toBeNull();
    page.stop();

    shared.refuse(false);
    const next = shared.page(legacy);
    await waitFor(next, (snapshot) => snapshot.context.views.length === 1);
    expect([names(next), legacy.stored]).toEqual([["Mine"], null]);
  });

  test("what the browser stored in another shape is no view to import, and is forgotten", async () => {
    const legacy = { stored: { version: 9, views: "nope" } as unknown };
    expect(legacyViews(legacy.stored)).toEqual([]);
    const page = server().page(legacy);
    await live(page);
    expect([names(page), legacy.stored]).toEqual([[], null]);
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

  describe("when the server does not keep a change", () => {
    async function refusing() {
      const shared = server([{ id: "v1", name: "Ready web", filter: "ready=true" }]);
      const page = shared.page();
      await live(page);
      await waitFor(page, (snapshot) => snapshot.context.views.length === 1);
      shared.refuse(true);
      return { shared, page };
    }

    test("a save, a rename and a delete are each refused and change no view", async () => {
      const { page } = await refusing();
      page.send({ type: "save", form: "save", name: "Mine", scope: ALL, filter: NO_FILTER });
      page.send({ type: "rename", form: "rename", id: "v1", name: "Web, ready" });
      page.send({ type: "remove", form: "remove", id: "v1" });
      expect(writeState(page.getSnapshot(), "save")).toBe("pending");
      await Promise.all(["save", "rename", "remove"].map((form) => settled(page, form)));
      const snapshot = page.getSnapshot();
      expect(["save", "rename", "remove"].map((form) => writeState(snapshot, form))).toEqual([
        "refused",
        "refused",
        "refused",
      ]);
      expect(names(page)).toEqual(["Ready web"]);
    });

    test("a refused rename stays refused when another form's save is kept, and the form's leaving forgets it", async () => {
      const { shared, page } = await refusing();
      page.send({ type: "rename", form: "rename", id: "v1", name: "Web, ready" });
      await settled(page, "rename");
      shared.refuse(false);
      page.send({ type: "save", form: "save", name: "Mine", scope: ALL, filter: NO_FILTER });
      await settled(page, "save");
      const snapshot = page.getSnapshot();
      expect([writeState(snapshot, "rename"), writeState(snapshot, "save")]).toEqual([
        "refused",
        "idle",
      ]);
      page.send({ type: "forget", form: "rename" });
      expect(writeState(page.getSnapshot(), "rename")).toBe("idle");
    });
  });
});
