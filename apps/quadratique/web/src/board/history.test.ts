import { describe, expect, test } from "vitest";

import { prepended } from "./history.ts";
import type { HistoryChange } from "./model.ts";

const change = (commit: string, issue = "DEM-1"): HistoryChange => ({
  commit,
  at: "2026-11-19T10:00:00Z",
  author: { kind: "human", name: "tester" },
  operation: "issue.update",
  project: "DEM",
  issue,
  fields: [],
});

describe("the history read again after a change", () => {
  test("puts only the newer changes before the known ones, and keeps the older pages read so far", () => {
    const known = { changes: [change("c3"), change("c2")], changesCursor: "older" };
    const page = { items: [change("c5"), change("c4"), change("c3")], nextCursor: "next" };
    expect(prepended(known, page)).toEqual({
      changes: [change("c5"), change("c4"), change("c3"), change("c2")],
      changesCursor: "older",
    });
  });

  test("replaces what it knew when more changes happened than one page holds", () => {
    const known = { changes: [change("c1")], changesCursor: undefined };
    const page = { items: [change("c9"), change("c8")], nextCursor: "c8" };
    expect(prepended(known, page)).toEqual({ changes: page.items, changesCursor: "c8" });
  });

  test("keeps the cursor of a full page that follows an empty history, so older changes can be shown", () => {
    const known = { changes: [], changesCursor: undefined };
    const page = { items: [change("c9"), change("c8")], nextCursor: "c8" };
    expect(prepended(known, page)).toEqual({ changes: page.items, changesCursor: "c8" });
  });
});
