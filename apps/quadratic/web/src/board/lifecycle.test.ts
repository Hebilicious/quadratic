import { expect, test } from "vitest";

import { summary } from "../test/fixtures.ts";
import { explainsNotReady, notReadyShown } from "./lifecycle.ts";

test("the page explains why an issue is not ready only while its work has not started", () => {
  expect(explainsNotReady(summary("DEM-1", { ready: false, notReady: ["blocked"] }))).toBe(true);
  expect(
    explainsNotReady(
      summary("DEM-1", {
        status: "done",
        category: "completed",
        ready: false,
        notReady: ["category"],
      }),
    ),
  ).toBe(false);
  expect(
    explainsNotReady(
      summary("DEM-1", {
        status: "doing",
        category: "started",
        ready: false,
        notReady: ["category", "mine"],
      }),
    ),
  ).toBe(false);
});

test("the page shows that an issue in progress waits on an issue it is stacked on that went back", () => {
  const backFromReview = summary("DEM-2", {
    status: "doing",
    category: "started",
    ready: false,
    notReady: ["category", "stacked", "mine"],
    blockedBy: ["DEM-1"],
  });
  expect(explainsNotReady(backFromReview)).toBe(true);
  expect(notReadyShown(backFromReview)).toEqual(["stacked"]);
  const unstarted = summary("DEM-3", { ready: false, notReady: ["stacked", "assigned"] });
  expect(notReadyShown(unstarted)).toEqual(["stacked", "assigned"]);
});
