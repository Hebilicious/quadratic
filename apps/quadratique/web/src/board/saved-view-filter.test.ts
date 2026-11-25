import { expect, test } from "vitest";

import { sharedCatalog } from "../core/catalog.ts";
import { LIMITS } from "../route.ts";
import { storedFilter } from "./saved-views.machine.ts";
import { CLAIM_FILTERS, type Filter } from "./view.ts";

/** The longest filter the page can hold: every list at its cap, each item at its longest, the search text in four-byte characters. */
function longestFilter(): Filter {
  const longest = (prefix: string, length: number) =>
    `${prefix}${"-a".repeat(length)}`.slice(0, length);
  const label = (index: number) =>
    `${String(index)}${":a".repeat(LIMITS.label.maxLength)}`.slice(0, LIMITS.label.maxLength);
  return {
    statuses: Array.from({ length: LIMITS.statuses }, (_, index) =>
      longest(`s${String(index)}`, LIMITS.status.maxLength),
    ),
    categories: ["backlog", "unstarted", "started", "completed", "canceled"],
    priorities: [0, 1, 2, 3, 4],
    labels: Array.from({ length: LIMITS.labels }, (_, index) => label(index)),
    assignee: { kind: "role", role: longest("r", LIMITS.role.maxLength) },
    claim: CLAIM_FILTERS.toSorted((left, right) => right.length - left.length)[0],
    ready: false,
    text: "😀".repeat(LIMITS.text),
  };
}

test("the longest filter the page can hold fits the catalog's limit for a saved view's filter", () => {
  const limit = sharedCatalog.$defs?.["SavedViewFilter"]?.["maxLength"];
  const stored = storedFilter(longestFilter());
  expect(stored.length).toBeGreaterThan(2000);
  expect(
    typeof limit === "number" && stored.length <= limit,
    `${String(stored.length)} > ${String(limit)}`,
  ).toBe(true);
});
