export type Key =
  | "j"
  | "k"
  | "h"
  | "l"
  | "H"
  | "L"
  | "x"
  | "space"
  | "enter"
  | "c"
  | "g"
  | "i"
  | "m"
  | "s"
  | "p"
  | "a"
  | "?"
  | "/"
  | "escape"
  | "mod+k"
  | "mod+b";

/** A select is its own kind: the browser makes it answer letters by choosing the option they start. */
export type Focus = "field" | "select" | "control" | "page";

export interface Pressed {
  readonly key: string;
  readonly shiftKey: boolean;
  readonly metaKey: boolean;
  readonly ctrlKey: boolean;
  readonly altKey: boolean;
}

const PLAIN: Readonly<Record<string, Key>> = {
  j: "j",
  ArrowDown: "j",
  k: "k",
  ArrowUp: "k",
  h: "h",
  ArrowLeft: "h",
  l: "l",
  ArrowRight: "l",
  x: "x",
  " ": "space",
  Enter: "enter",
  c: "c",
  g: "g",
  i: "i",
  m: "m",
  s: "s",
  p: "p",
  a: "a",
  "?": "?",
  "/": "/",
  Escape: "escape",
};

const SHIFTED: Readonly<Record<string, Key>> = {
  H: "H",
  ArrowLeft: "H",
  L: "L",
  ArrowRight: "L",
  "?": "?",
};

const NATIVE_ON_CONTROLS: readonly string[] = [
  " ",
  "Enter",
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
];

export function shortcutOf(pressed: Pressed, focus: Focus): Key | undefined {
  if (pressed.altKey) {
    return undefined;
  }
  if (pressed.metaKey || pressed.ctrlKey) {
    const letter = pressed.key.toLowerCase();
    return letter === "k" ? "mod+k" : letter === "b" && focus !== "field" ? "mod+b" : undefined;
  }
  if (focus === "field" || focus === "select") {
    return undefined;
  }
  if (focus === "control" && NATIVE_ON_CONTROLS.includes(pressed.key)) {
    return undefined;
  }
  return pressed.shiftKey ? SHIFTED[pressed.key] : PLAIN[pressed.key];
}

export function focusOf(element: Element | null): Focus {
  if (element === null) {
    return "page";
  }
  if (
    element instanceof HTMLInputElement ||
    element instanceof HTMLTextAreaElement ||
    (element instanceof HTMLElement && element.isContentEditable)
  ) {
    return "field";
  }
  if (element instanceof HTMLSelectElement) {
    return "select";
  }
  if (
    element instanceof HTMLButtonElement ||
    element instanceof HTMLAnchorElement ||
    element.tagName === "SUMMARY"
  ) {
    return "control";
  }
  return "page";
}

export const SHORTCUTS: readonly {
  readonly keys: readonly string[];
  readonly join: "or" | "then";
  readonly does: string;
}[] = [
  { keys: ["J", "K"], join: "or", does: "Move down and up" },
  { keys: ["H", "L"], join: "or", does: "Move to the column on the left or right (board)" },
  {
    keys: ["Shift H", "Shift L"],
    join: "or",
    does: "Move the selected issues to the previous or next status",
  },
  { keys: ["X"], join: "or", does: "Select or unselect the issue" },
  { keys: ["Space"], join: "or", does: "Peek at the issue beside the list" },
  { keys: ["Enter"], join: "or", does: "Open the issue" },
  { keys: ["C"], join: "or", does: "Create an issue" },
  { keys: ["S"], join: "or", does: "Set the status" },
  { keys: ["P"], join: "or", does: "Set the priority" },
  { keys: ["A"], join: "or", does: "Assign" },
  { keys: ["Mod K"], join: "or", does: "Open the command menu" },
  { keys: ["Mod B"], join: "or", does: "Switch between list and board" },
  { keys: ["G", "I"], join: "then", does: "Go to what waits on you" },
  { keys: ["G", "M"], join: "then", does: "Go to what every agent is doing" },
  { keys: ["G", "H"], join: "then", does: "Go to the history of changes" },
  { keys: ["/"], join: "or", does: "Search the issues shown" },
  { keys: ["?"], join: "or", does: "Show the shortcuts" },
  { keys: ["Esc"], join: "or", does: "Close what is open, or clear the selection" },
];

export const MOD_KEY: string = /Mac|iPhone|iPad/u.test(globalThis.navigator?.platform ?? "")
  ? "⌘"
  : "Ctrl";

export function keyLabel(keys: string): string {
  return keys.replace("Mod", MOD_KEY);
}
