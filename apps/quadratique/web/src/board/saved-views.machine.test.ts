import { describe, expect, test } from "vitest";
import { createActor, waitFor } from "xstate";

import { batches, IMPORT_LIMITS, planLegacy, type OldView } from "./legacy-views.ts";
import {
  refusalOf,
  savedViewOf,
  savedViewsMachine,
  viewsReady,
  writeState,
  type Imported,
  type NewView,
  type Snapshot,
  type StoredView,
} from "./saved-views.machine.ts";
import { ALL, NO_FILTER, projectScope } from "./view.ts";

const MOST_VIEWS = 100;

class Refusal extends Error {
  constructor(readonly failure: { readonly _tag: string }) {
    super(failure._tag);
  }
}

interface Deferred<T> {
  readonly promise: Promise<T>;
  readonly resolve: (value: T) => void;
  readonly reject: (error: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve: (value: T) => void = () => {};
  let reject: (error: unknown) => void = () => {};
  const promise = new Promise<T>((onValue, onError) => {
    resolve = onValue;
    reject = onError;
  });
  return { promise, resolve, reject };
}

/**
 * A server with the catalog's rules: a revision that grows with each change, create or replace by intent, receipts per
 * imported old view, at most 100 views. Tests can hold a list answer or a write answer, refuse calls, and see each call.
 */
function server(initial: readonly StoredView[] = []) {
  let views: StoredView[] = [...initial];
  let next = initial.length + 1;
  let revision = initial.length;
  const receipts = new Set<string>();
  const channels = new Set<(snapshot: Snapshot) => void>();
  const calls: string[] = [];
  const control = {
    refuses: false,
    heldLists: [] as Deferred<void>[],
    holdLists: false,
    failLists: 0,
    holdWrites: false,
    heldWrites: [] as Deferred<void>[],
    firstHeld: deferred<void>(),
    failImportsAfter: Number.POSITIVE_INFINITY,
  };
  const sameName = (left: string, right: string) =>
    left.trim().toLowerCase() === right.trim().toLowerCase();
  const current = (): Snapshot => ({ revision, items: [...views] });
  const commit = (change: () => void) => {
    const before = JSON.stringify(views);
    change();
    if (JSON.stringify(views) !== before) {
      revision += 1;
      for (const channel of channels) {
        channel(current());
      }
    }
  };
  const add = (view: NewView): string => {
    const id = `v${String(next)}`;
    next += 1;
    views.push({ id, ...view });
    return id;
  };
  const write = async <T>(name: string, change: () => T): Promise<T> => {
    calls.push(name);
    await Promise.resolve();
    if (control.refuses) {
      throw new Error("the server did not answer");
    }
    let answer: T | undefined;
    commit(() => {
      answer = change();
    });
    if (control.holdWrites) {
      const held = deferred<void>();
      control.heldWrites.push(held);
      control.firstHeld.resolve();
      await held.promise;
    }
    if (answer === undefined) {
      throw new Error("no answer");
    }
    return answer;
  };
  return {
    control,
    calls,
    get views() {
      return views;
    },
    get channels() {
      return channels.size;
    },
    page(storage: { value: string | null } = { value: null }) {
      let sources = 0;
      const machine = savedViewsMachine({
        list: async () => {
          calls.push("list");
          const answer = current();
          await Promise.resolve();
          if (control.failLists > 0) {
            control.failLists -= 1;
            throw new Error("the list did not come");
          }
          if (control.holdLists) {
            const held = deferred<void>();
            control.heldLists.push(held);
            await held.promise;
          }
          return answer;
        },
        save: (view) =>
          write("save", () => {
            const { replace, ...fields } = view;
            if (replace === undefined) {
              const named = views.find((each) => sameName(each.name, view.name));
              if (named !== undefined) {
                throw new Refusal({ _tag: "NameTaken" });
              }
              add(fields);
            } else {
              const replaced = views.find((each) => each.id === replace);
              if (replaced === undefined) {
                throw new Refusal({ _tag: "NotFound" });
              }
              if (!sameName(replaced.name, view.name)) {
                throw new Refusal({ _tag: "Renamed" });
              }
              views = views.map((each) =>
                each.id === replace ? { ...fields, id: each.id, name: each.name } : each,
              );
            }
            return current();
          }),
        rename: (id, name) =>
          write("rename", () => {
            views = views.map((each) => (each.id === id ? { ...each, name } : each));
            return current();
          }),
        remove: (id) =>
          write("remove", () => {
            views = views.filter((each) => each.id !== id);
            return current();
          }),
        import: (source, imported) => {
          if (control.failImportsAfter <= 0) {
            calls.push("import");
            return Promise.reject(new Error("the server did not answer"));
          }
          control.failImportsAfter -= 1;
          return write("import", (): Imported => {
            const full: string[] = [];
            for (const old of imported) {
              const receipt = `${source}/${old.old}`;
              if (receipts.has(receipt)) {
                continue;
              }
              if (views.length >= MOST_VIEWS) {
                full.push(old.old);
                continue;
              }
              receipts.add(receipt);
              const { old: _, ...view } = old;
              add(view);
            }
            return { ...current(), full };
          });
        },
        changes: async (signal) => {
          await Promise.resolve();
          if (control.refuses) {
            throw new Error("the server did not answer");
          }
          const queue: Snapshot[] = [];
          let wake: (() => void) | undefined;
          const channel = (snapshot: Snapshot) => {
            queue.push(snapshot);
            wake?.();
          };
          channels.add(channel);
          signal.addEventListener("abort", () => {
            channels.delete(channel);
            wake?.();
          });
          return (async function* () {
            while (!signal.aborted) {
              const snapshot = queue.shift();
              if (snapshot === undefined) {
                await new Promise<void>((resolve) => {
                  wake = resolve;
                });
              } else {
                yield snapshot;
              }
            }
          })();
        },
        legacy: {
          read: () => storage.value,
          write: (value) => {
            storage.value = value;
          },
          removeIf: (value) => {
            if (storage.value === value) {
              storage.value = null;
            }
          },
        },
        newSource: () => {
          sources += 1;
          return `source-${String(sources)}`;
        },
      }).provide({ delays: { retry: 0 } });
      const actor = createActor(machine);
      actor.start();
      return actor;
    },
  };
}

type Page = ReturnType<ReturnType<typeof server>["page"]>;

const live = (page: Page) => waitFor(page, (snapshot) => viewsReady(snapshot));
const names = (page: Page) => page.getSnapshot().context.views.map((view) => view.name);
const settled = (page: Page, form: string) =>
  waitFor(page, (snapshot) => writeState(snapshot, form) !== "pending");
const legacy = (views: readonly object[], extra: object = {}) =>
  JSON.stringify({ version: 1, ...extra, views });

const ready = { ...NO_FILTER, labels: ["web"], ready: true };

describe("saved views", () => {
  test("a view saved, renamed and deleted in one page is what another page of the same server shows", async () => {
    const shared = server();
    const first = shared.page();
    const second = shared.page();
    await Promise.all([live(first), live(second)]);

    const save = (name: string, filter = NO_FILTER) => {
      first.send({
        type: "save",
        form: "s",
        name,
        scope: projectScope("DEM"),
        filter,
        replace: undefined,
      });
      return settled(first, "s");
    };
    await save(" Ready web ", ready);
    await save("Mine");
    first.send({ type: "rename", form: "r", id: "v1", name: "Web, ready" });
    await settled(first, "r");
    first.send({ type: "remove", form: "d", id: "v2" });
    await settled(first, "d");

    await waitFor(second, (snapshot) => snapshot.context.revision === 4);
    expect(second.getSnapshot().context.views).toEqual([
      { id: "v1", name: "Web, ready", scope: projectScope("DEM"), filter: ready },
    ]);
  });

  test("renaming to a name another view has, or to an empty one, sends nothing", async () => {
    const shared = server([
      { id: "v1", name: "Web", filter: "" },
      { id: "v2", name: "Other", filter: "" },
    ]);
    const page = shared.page();
    await live(page);
    page.send({ type: "rename", form: "r", id: "v2", name: "WEB" });
    page.send({ type: "rename", form: "r", id: "v2", name: "  " });
    expect([writeState(page.getSnapshot(), "r"), names(page)]).toEqual(["idle", ["Web", "Other"]]);
  });

  test("a list that settles after a newer event leaves the newer views", async () => {
    const shared = server([{ id: "v1", name: "Before", filter: "" }]);
    const writer = shared.page();
    await live(writer);
    shared.control.holdLists = true;
    const page = shared.page();
    await waitFor(page, (snapshot) => snapshot.matches({ following: "listing" }));

    writer.send({ type: "rename", form: "r", id: "v1", name: "After" });
    await waitFor(page, (snapshot) => snapshot.context.revision === 2);
    expect(names(page)).toEqual(["After"]);
    for (const held of shared.control.heldLists) {
      held.resolve();
    }
    await live(page);

    expect([names(page), page.getSnapshot().context.revision]).toEqual([["After"], 2]);
  });

  test("a write's answer that comes after a newer event leaves the newer views", async () => {
    const shared = server();
    const first = shared.page();
    const second = shared.page();
    await Promise.all([live(first), live(second)]);
    shared.control.holdWrites = true;
    first.send({
      type: "save",
      form: "s",
      name: "Mine",
      scope: ALL,
      filter: NO_FILTER,
      replace: undefined,
    });
    await waitFor(second, (snapshot) => snapshot.context.revision === 1);
    shared.control.holdWrites = false;
    second.send({ type: "rename", form: "r", id: "v1", name: "Ours" });
    await waitFor(first, (snapshot) => snapshot.context.revision === 2);

    for (const held of shared.control.heldWrites) {
      held.resolve();
    }
    await settled(first, "s");

    expect([names(first), first.getSnapshot().context.revision]).toEqual([["Ours"], 2]);
  });

  test("a list that fails ends the live channel, and the page reconnects and is ready only with a list", async () => {
    const shared = server([{ id: "v1", name: "Mine", filter: "" }]);
    shared.control.failLists = 1;
    const page = shared.page();
    await waitFor(page, (snapshot) => snapshot.matches("reconnecting"));
    expect([viewsReady(page.getSnapshot()), shared.channels]).toEqual([false, 0]);

    await live(page);

    expect([
      names(page),
      shared.calls.filter((call) => call === "list").length,
      shared.channels,
    ]).toEqual([["Mine"], 2, 1]);
  });

  test("a save says whether it creates or replaces, and a conflict is named for the form", async () => {
    const shared = server([{ id: "v1", name: "Mine", filter: "claim=mine" }]);
    const page = shared.page();
    await live(page);

    page.send({
      type: "save",
      form: "new",
      name: "mine",
      scope: ALL,
      filter: ready,
      replace: undefined,
    });
    await settled(page, "new");
    page.send({
      type: "save",
      form: "moved",
      name: "Theirs",
      scope: ALL,
      filter: ready,
      replace: "v1",
    });
    await settled(page, "moved");
    page.send({
      type: "save",
      form: "same",
      name: "Mine",
      scope: ALL,
      filter: ready,
      replace: "v1",
    });
    await settled(page, "same");

    const snapshot = page.getSnapshot();
    expect([
      refusalOf(snapshot, "new"),
      refusalOf(snapshot, "moved"),
      refusalOf(snapshot, "same"),
    ]).toEqual(["NameTaken", "Renamed", undefined]);
    expect(shared.views).toEqual([{ id: "v1", name: "Mine", filter: "label=web&ready=true" }]);
  });

  describe("the views this browser kept before the server kept them", () => {
    test("are imported once with an id of this browser's views, and then removed from the browser", async () => {
      const shared = server([{ id: "v1", name: "Mine", filter: "ready=false" }]);
      const storage = {
        value: legacy([
          { id: "v1", name: "Mine", filter: "ready=true" },
          { id: "v2", name: "Mine (imported)", filter: "ready=true" },
        ]),
      };
      const page = shared.page(storage);
      await live(page);
      // Two different old views stay two views; the server's names for them are tested with the server.
      expect(page.getSnapshot().context.views).toHaveLength(3);
      expect(shared.calls.filter((call) => call === "import")).toHaveLength(1);
      expect(storage.value).toBeNull();
    });

    test("that stop half-way go on at the next page without making a view twice", async () => {
      const shared = server();
      const old = Array.from({ length: 90 }, (_, index) => ({
        id: `v${String(index + 1)}`,
        name: `View ${String(index + 1)}`,
        filter: `text=${"x".repeat(IMPORT_LIMITS.filter - 5)}`,
      }));
      const storage: { value: string | null } = { value: legacy(old) };
      shared.control.failImportsAfter = 1;
      const first = shared.page(storage);
      await live(first);
      const halfWay = shared.views.length;
      expect([first.getSnapshot().context.report.interrupted, halfWay > 0, halfWay < 90]).toEqual([
        true,
        true,
        true,
      ]);
      expect(storage.value).not.toBeNull();
      first.stop();

      shared.control.failImportsAfter = Number.POSITIVE_INFINITY;
      const second = shared.page(storage);
      await live(second);
      expect([shared.views.length, new Set(shared.views.map((view) => view.name)).size]).toEqual([
        90, 90,
      ]);
      expect(storage.value).toBeNull();
    });

    test("over the most views stay in the browser, said and not lost", async () => {
      const shared = server();
      const old = Array.from({ length: 120 }, (_, index) => ({
        id: `v${String(index + 1)}`,
        name: `View ${String(index + 1)}`,
        filter: "",
      }));
      const storage: { value: string | null } = { value: legacy(old) };
      const page = shared.page(storage);
      await live(page);
      const { report } = page.getSnapshot().context;
      expect([shared.views.length, report.kept.length, report.kept[0]?.name]).toEqual([
        100,
        20,
        "View 101",
      ]);
      expect(shared.calls.filter((call) => call === "import")).toHaveLength(2);
      expect(storage.value).not.toBeNull();
    });

    test("a name over the longest name is saved cut on a character boundary, and the page says so", async () => {
      const shared = server();
      const long = "😀".repeat(120);
      const page = shared.page({ value: legacy([{ id: "v1", name: long, filter: "" }]) });
      await live(page);
      const [view] = shared.views;
      expect([...(view?.name ?? "")]).toHaveLength(IMPORT_LIMITS.name);
      expect(page.getSnapshot().context.report.shortened).toEqual([{ from: long, to: view?.name }]);
    });

    test("a value of a shape this page does not know stays, and nothing is sent", async () => {
      const shared = server();
      const unknown = JSON.stringify({ version: 2, views: [{ id: "v1", name: "Later" }] });
      const storage = { value: unknown };
      const page = shared.page(storage);
      await live(page);
      expect([storage.value, shared.calls.includes("import")]).toEqual([unknown, false]);
    });

    test("a value another tab changed while the import ran is not removed", async () => {
      const shared = server();
      const storage: { value: string | null } = {
        value: legacy([{ id: "v1", name: "Mine", filter: "" }]),
      };
      shared.control.holdWrites = true;
      const page = shared.page(storage);
      await shared.control.firstHeld.promise;
      const newer = legacy([{ id: "v9", name: "Newer", filter: "" }]);
      storage.value = newer;
      shared.control.holdWrites = false;
      for (const held of shared.control.heldWrites) {
        held.resolve();
      }
      await live(page);
      expect(storage.value).toBe(newer);
    });
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
      shared.control.refuses = true;
      return { shared, page };
    }

    test("a save, a rename and a delete are each refused and change no view", async () => {
      const { page } = await refusing();
      page.send({
        type: "save",
        form: "save",
        name: "Mine",
        scope: ALL,
        filter: NO_FILTER,
        replace: undefined,
      });
      page.send({ type: "rename", form: "rename", id: "v1", name: "Web, ready" });
      page.send({ type: "remove", form: "remove", id: "v1" });
      expect(writeState(page.getSnapshot(), "save")).toBe("pending");
      await Promise.all(["save", "rename", "remove"].map((form) => settled(page, form)));
      const snapshot = page.getSnapshot();
      expect(["save", "rename", "remove"].map((form) => refusalOf(snapshot, form))).toEqual([
        "unavailable",
        "unavailable",
        "unavailable",
      ]);
      expect(names(page)).toEqual(["Ready web"]);
    });

    test("a refused rename stays refused when another form's save is kept, and the form's leaving forgets it", async () => {
      const { shared, page } = await refusing();
      page.send({ type: "rename", form: "rename", id: "v1", name: "Web, ready" });
      await settled(page, "rename");
      shared.control.refuses = false;
      page.send({
        type: "save",
        form: "save",
        name: "Mine",
        scope: ALL,
        filter: NO_FILTER,
        replace: undefined,
      });
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

describe("the plan of an import", () => {
  test("calls hold at most the catalog's number of views and stay far under the request limit", () => {
    const small: OldView[] = Array.from({ length: 250 }, (_, index) => ({
      old: `v${String(index)}`,
      name: "n",
      filter: "",
    }));
    expect(batches(small).map((batch) => batch.length)).toEqual([100, 100, 50]);
    const large: OldView[] = Array.from({ length: 100 }, (_, index) => ({
      old: `v${String(index)}`,
      name: "😀".repeat(IMPORT_LIMITS.name),
      filter: "x".repeat(IMPORT_LIMITS.filter),
    }));
    const sizes = batches(large).map(
      (batch) =>
        new TextEncoder().encode(JSON.stringify({ source: "s".repeat(64), views: batch })).length,
    );
    expect(sizes.length).toBeGreaterThan(1);
    expect(sizes.every((size) => size < 1024 * 1024)).toBe(true);
  });

  test("a row the server would refuse stays in the browser with its reason, and the others go", () => {
    const plan = planLegacy(
      legacy([
        { id: "v1", name: "Fine", filter: "" },
        { id: "v2", name: "Lower", project: "dem", filter: "" },
        { id: "v3", name: "Long", filter: "x".repeat(IMPORT_LIMITS.filter + 1) },
        { id: "v4", name: "   ", filter: "" },
        { id: "v1", name: "Twice", filter: "" },
      ]),
    );
    expect(
      plan.kind === "import"
        ? [plan.rows.map((row) => row.name), plan.kept.map((kept) => kept.name)]
        : plan,
    ).toEqual([["Fine"], ["Lower", "Long", "v4", "Twice"]]);
  });
});
