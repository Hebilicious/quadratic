import { describe, expect, test } from "vitest";

import { changeLines, relationLabel, type ChangeLine } from "./present.ts";

const statusName = (key: string) => key;
const shown = (line: ChangeLine) => `${line.before ?? ""} → ${line.after ?? ""}`;

describe("a change of the history, in plain words", () => {
  test("a long value changed at its end is folded behind its label and never reads the same before and after", () => {
    const opening = "The board shows every issue of the project. ".repeat(4);
    const lines = changeLines(
      [
        {
          field: "description",
          before: JSON.stringify(`${opening}Old ending.`),
          after: JSON.stringify(`${opening}New ending.`),
        },
      ],
      statusName,
    );
    expect(lines.map((line) => [line.before === line.after, line.long])).toEqual([[false, true]]);
    expect(lines.map(shown).join("")).toContain("New ending.");
  });

  test("a change of the order alone has a line of its own", () => {
    const lines = changeLines([{ field: "order", before: "1", after: "3" }], statusName);
    expect(lines.length).toBe(1);
  });

  test("any two different stored texts read differently on screen and to a screen reader, whatever their whitespace", () => {
    const alphabet = ["a", " ", "\t", "\n", "\r", "\u00a0", "\u2028", "\u0000", "\\", "u", "0"];
    const texts = [
      "",
      ...alphabet,
      ...alphabet.flatMap((first) => alphabet.map((second) => first + second)),
    ];
    const read = (text: string | undefined) => (text ?? "").replace(/\s+/gu, " ").trim();
    const same: string[] = [];
    for (const before of texts) {
      for (const after of texts) {
        if (before === after) {
          continue;
        }
        const [line] = changeLines(
          [{ field: "title", before: JSON.stringify(before), after: JSON.stringify(after) }],
          statusName,
        );
        if (read(line?.before) === read(line?.after)) {
          same.push(JSON.stringify([before, after]));
        }
      }
    }
    expect(same.slice(0, 5)).toEqual([]);
  });

  test("two statuses whose names differ only in spaces read as their keys", () => {
    const names: Record<string, string> = { review: "In  review", checking: "In review" };
    const [line] = changeLines(
      [{ field: "status", before: JSON.stringify("review"), after: JSON.stringify("checking") }],
      (key) => names[key] ?? key,
    );
    expect([line?.before, line?.after]).toEqual(['"review"', '"checking"']);
  });
});

test("a stacked-on relation reads from both of its issues", () => {
  expect(relationLabel({ kind: "stacked-on", direction: "outgoing" })).toBe("Stacked on");
  expect(relationLabel({ kind: "stacked-on", direction: "incoming" })).toBe("Base of");
});
