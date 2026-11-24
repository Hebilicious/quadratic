import { describe, expect, test } from "vitest";
import { createActor } from "xstate";

import { followed, horizontal, vertical } from "./cursor.ts";
import { shortcutOf, type Pressed } from "./keys.ts";
import { keysMachine, type KeyEffects } from "./keys.machine.ts";

const press = (key: string, extra: Partial<Pressed> = {}): Pressed => ({
  key,
  shiftKey: false,
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  ...extra,
});

describe("shortcuts", () => {
  test("no shortcut fires while the person types in a field, except the command menu", () => {
    for (const key of ["j", "k", "x", "c", "g", " ", "Enter", "?", "Escape"]) {
      expect(shortcutOf(press(key), "field"), key).toBeUndefined();
    }
    expect(shortcutOf(press("b", { ctrlKey: true }), "field")).toBeUndefined();
    expect(shortcutOf(press("k", { ctrlKey: true }), "field")).toBe("mod+k");
    expect(shortcutOf(press("k", { metaKey: true }), "field")).toBe("mod+k");
    expect(shortcutOf(press("j"), "page")).toBe("j");
    expect(shortcutOf(press("ArrowRight", { shiftKey: true }), "page")).toBe("L");
    expect(shortcutOf(press("Enter"), "control")).toBeUndefined();
    expect(shortcutOf(press("ArrowDown"), "control")).toBeUndefined();
    expect(shortcutOf(press("j"), "control")).toBe("j");
  });
});

describe("the cursor on a board", () => {
  const board = [["A1", "A2", "A3"], [], ["C1"], ["D1", "D2"]];

  test("J and K move within a column and stop at its ends; H and L skip empty columns at the same height", () => {
    expect(vertical(board, undefined, 1)).toBe("A1");
    expect(vertical(board, "A1", 1)).toBe("A2");
    expect(vertical(board, "A3", 1)).toBe("A3");
    expect(vertical(board, "A1", -1)).toBe("A1");
    expect(horizontal(board, "A3", 1)).toBe("C1");
    expect(horizontal(board, "C1", 1)).toBe("D1");
    expect(horizontal(board, "D2", -1)).toBe("C1");
    expect(horizontal(board, "D2", 1)).toBe("D2");
  });

  test("the cursor follows its issue to another column, takes its place when it leaves, and waits for an issue not shown yet", () => {
    const moved = [["A1", "A3"], ["A2"], ["C1"], ["D1", "D2"]];
    expect(followed(board, moved, "A2")).toBe("A2");
    const gone = [["A1", "A3"], [], ["C1"], ["D1", "D2"]];
    expect(followed(board, gone, "A2")).toBe("A3");
    expect(followed(board, board, "NEW-9")).toBe("NEW-9");
  });
});

describe("the keyboard", () => {
  function keys() {
    const went: string[] = [];
    const effects: KeyEffects = {
      goToWaiting: () => went.push("waiting"),
      goToNow: () => went.push("now"),
      goToHistory: () => went.push("history"),
      focusSearch: () => went.push("search"),
      open: (issue) => went.push(`open ${issue}`),
      switchView: () => went.push("switch"),
      shift: (issues, by) => went.push(`shift ${issues.join(",")} ${String(by)}`),
    };
    const actor = createActor(keysMachine(effects));
    actor.start();
    actor.send({
      type: "layout",
      surface: "board",
      moment: "now",
      layout: [["A1", "A2"], ["B1"]],
    });
    return { actor, went };
  }

  test("G then I goes to what waits on the human, G then M to the now view, G then H to the history; G then another key goes nowhere", () => {
    const { actor, went } = keys();
    for (const key of ["g", "i", "g", "m", "g", "h", "g", "j", "i"] as const) {
      actor.send({ type: "key", key });
    }
    expect(went).toEqual(["waiting", "now", "history"]);
    actor.stop();
  });

  test("on the board as it was, the cursor moves but nothing is selected, moved, peeked at or created", () => {
    const { actor, went } = keys();
    actor.send({
      type: "layout",
      surface: "board",
      moment: "past",
      layout: [["A1", "A2"], ["B1"]],
    });
    for (const key of ["j", "x", "L", "space", "c"] as const) {
      actor.send({ type: "key", key });
    }
    const { context } = actor.getSnapshot();
    expect([went, context.cursor, context.selected]).toEqual([[], "A1", []]);
    expect(actor.getSnapshot().matches({ peek: "closed", overlay: "none" })).toBe(true);
    actor.stop();
  });

  test("with the peek open, a move to the board as it was closes the create form and the peek, and knows it is the past", () => {
    const { actor } = keys();
    actor.send({ type: "key", key: "j" });
    actor.send({ type: "peek" });
    actor.send({ type: "openCreate" });
    actor.send({
      type: "layout",
      surface: "board",
      moment: "past",
      layout: [["A1", "A2"], ["B1"]],
    });
    const snapshot = actor.getSnapshot();
    expect([
      snapshot.matches({ overlay: "none", peek: "closed" }),
      snapshot.context.moment,
    ]).toEqual([true, "past"]);
    actor.stop();
  });

  test("a move to the board as it was, such as by the browser's Back, closes the create form and the command menu", () => {
    const past = { type: "layout", surface: "board", moment: "past", layout: [["A1"]] } as const;
    for (const open of [{ type: "openCreate" }, { type: "openMenu", query: "" }] as const) {
      const { actor } = keys();
      actor.send(open);
      actor.send(past);
      actor.send({ type: "openCreate" });
      expect(actor.getSnapshot().matches({ overlay: "none" }), open.type).toBe(true);
      actor.stop();
    }
  });

  test("X selects several issues, and moving them acts on every selected one; Escape clears the selection", () => {
    const { actor, went } = keys();
    for (const key of ["j", "x", "l", "x", "L"] as const) {
      actor.send({ type: "key", key });
    }
    expect(went).toEqual(["shift A1,B1 1"]);
    actor.send({ type: "key", key: "escape" });
    actor.send({ type: "key", key: "H" });
    expect(went.at(-1)).toBe("shift B1 -1");
    actor.stop();
  });

  test("while the command menu is open, the issue shortcuts do nothing, and Escape closes it", () => {
    const { actor, went } = keys();
    actor.send({ type: "key", key: "j" });
    actor.send({ type: "key", key: "mod+k" });
    actor.send({ type: "key", key: "enter" });
    actor.send({ type: "key", key: "L" });
    expect(went).toEqual([]);
    actor.send({ type: "key", key: "escape" });
    actor.send({ type: "key", key: "enter" });
    expect(went).toEqual(["open A1"]);
    actor.stop();
  });
});
