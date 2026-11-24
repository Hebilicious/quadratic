import { expect, test } from "vitest";

import { summary } from "../test/fixtures.ts";
import { explainsNotReady } from "./lifecycle.ts";

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
