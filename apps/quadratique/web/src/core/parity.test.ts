import * as App from "@acid-cat/core-effect/App";
import * as Handles from "@acid-cat/core-effect/Handles";
import type * as Http from "@acid-cat/core-effect/Http";
import type { Occurrence, Outcome } from "@acid-cat/spec-ts";
import { Effect, Layer, Stream } from "effect";
import { describe, expect, onTestFinished, test } from "vitest";

import { summary } from "../test/fixtures.ts";
import { app, PAGE_TARGETS, sharedCatalog } from "./catalog.ts";
import { Server } from "./server.ts";

const onThePage = (audience: readonly string[] | undefined) =>
  (audience ?? []).some((target) => PAGE_TARGETS.includes(target));

test("the page's core registers the shared catalog's targets, and every capability and event of web and webmcp", () => {
  const page = App.document(app);
  expect(page.targets).toEqual(sharedCatalog.targets);
  expect(Object.keys(page.capabilities).toSorted()).toEqual(
    Object.entries(sharedCatalog.capabilities)
      .filter(([, capability]) => onThePage(capability.audience))
      .map(([id]) => id)
      .toSorted(),
  );
  expect(Object.keys(page.events ?? {}).toSorted()).toEqual(
    Object.entries(sharedCatalog.events ?? {})
      .filter(([, event]) => onThePage(event.audience))
      .map(([id]) => id)
      .toSorted(),
  );
});

function server(answers: {
  readonly outcome: Outcome<unknown, unknown>;
  readonly occurrence: Occurrence;
}) {
  const forwarded: { readonly id: string; readonly input: unknown }[] = [];
  const client: Http.Client = {
    base: "http://127.0.0.1/web",
    capabilities: Effect.succeed([]),
    invoke: (id, input) =>
      Effect.sync(() => {
        forwarded.push({ id, input });
        return answers.outcome;
      }),
    subscribe: () => Effect.succeed(Stream.make(answers.occurrence)),
  };
  const layer = Layer.succeed(
    Server,
    Server.of({ client: () => client, listenerRegistered: () => undefined }),
  );
  return { layer, forwarded };
}

const occurrence = (data: unknown): Occurrence => ({
  name: "issue.changed",
  eventId: "c1",
  timestamp: "2026-11-18T10:00:00.000Z",
  cursor: "c1",
  data,
});

async function open(answers: Parameters<typeof server>[0]) {
  const fake = server(answers);
  const page = await Handles.open(app, fake.layer, { target: "web" });
  onTestFinished(async () => {
    await page.dispose();
  });
  const handle = (id: string) => {
    const found = page.handles.get(id);
    if (found === undefined) {
      throw new Error(`no capability ${id}`);
    }
    return found;
  };
  return { page, handle, forwarded: fake.forwarded };
}

const settled = async (promise: Promise<unknown>) =>
  await promise.then(
    (value) => ({ settled: "answered" as const, value }),
    (error: unknown) => ({ settled: "thrown" as const, error }),
  );

describe("the page's core holds every call and answer to the catalog's schemas", () => {
  const anything: Parameters<typeof server>[0] = {
    outcome: { ok: true, value: { issue: summary("DEM-1") } },
    occurrence: occurrence({}),
  };

  test("an update with the issue, its revision and a title is sent to the server", async () => {
    const { handle, forwarded } = await open({
      ...anything,
      outcome: {
        ok: true,
        value: {
          issue: { ...summary("DEM-1", { revision: 3 }), updatedAt: "2026-11-18T10:00:00Z" },
        },
      },
    });
    const result = await handle("issue.update").invokeExternal({
      issue: "DEM-1",
      revision: 2,
      title: "Write the guide",
    });
    expect(result).toMatchObject({ ok: true, value: { issue: { id: "DEM-1", revision: 3 } } });
    expect(forwarded).toEqual([
      { id: "issue.update", input: { issue: "DEM-1", revision: 2, title: "Write the guide" } },
    ]);
  });

  test.each([
    ["without its revision", { issue: "DEM-1", title: "Write the guide" }],
    [
      "with an issue id that is not one",
      { issue: "dem one", revision: 2, title: "Write the guide" },
    ],
  ])("an update %s is refused before it reaches the server", async (_name, input) => {
    const { handle, forwarded } = await open(anything);
    const result = await settled(handle("issue.update").invokeExternal(input));
    expect(result).not.toMatchObject({ settled: "answered", value: { ok: true } });
    expect(forwarded).toEqual([]);
  });

  test("a member the catalog does not have never reaches the server", async () => {
    const { handle, forwarded } = await open(anything);
    await settled(
      handle("issue.update").invokeExternal({ issue: "DEM-1", revision: 2, colour: "red" }),
    );
    expect(forwarded.map((each) => each.input)).not.toContainEqual(
      expect.objectContaining({ colour: "red" }),
    );
  });

  test("an answer of the server that lacks a member the catalog requires is not given to the page", async () => {
    const { handle } = await open({
      ...anything,
      outcome: { ok: true, value: { issue: { id: "DEM-1", project: "DEM" } } },
    });
    const result = await settled(handle("issue.get").invokeExternal({ issue: "DEM-1" }));
    expect(result).not.toMatchObject({ settled: "answered", value: { ok: true } });
  });

  test("an occurrence of issue.changed whose payload breaks the catalog's schema ends the subscription with a failure", async () => {
    const { page } = await open({ ...anything, occurrence: occurrence({ commit: "c1" }) });
    const event = page.events.get("issue.changed");
    if (event === undefined) {
      throw new Error("no event issue.changed");
    }
    const occurrences = await event.subscribe({});
    const first = await settled(occurrences[Symbol.asyncIterator]().next());
    expect(first.settled).toBe("thrown");
  });
});
