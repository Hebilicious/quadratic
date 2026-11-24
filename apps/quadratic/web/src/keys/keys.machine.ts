import { setup, types } from "xstate";

import { contains, followed, horizontal, vertical, type Layout } from "./cursor.ts";
import type { Key } from "./keys.ts";

export type Surface = "list" | "board" | "other";

export type Moment = "now" | "past";

export interface KeyEffects {
  readonly goToWaiting: () => void;
  readonly goToNow: () => void;
  readonly goToHistory: () => void;
  readonly focusSearch: () => void;
  readonly open: (issue: string) => void;
  readonly switchView: () => void;
  readonly shift: (issues: readonly string[], by: -1 | 1) => void;
}

interface KeysContext {
  readonly surface: Surface;
  readonly moment: Moment;
  readonly layout: Layout;
  readonly cursor: string | undefined;
  readonly selected: readonly string[];
  readonly menuQuery: string;
}

const GO_PREFIX_MS = 1500;

export function targets(context: Pick<KeysContext, "cursor" | "selected">): readonly string[] {
  if (context.selected.length > 0) {
    return context.selected;
  }
  return context.cursor === undefined ? [] : [context.cursor];
}

const toggled = (selected: readonly string[], id: string): readonly string[] =>
  selected.includes(id) ? selected.filter((each) => each !== id) : [...selected, id];

export function keysMachine(effects: KeyEffects) {
  return setup({
    schemas: {
      context: types<KeysContext>(),
      events: {
        key: types<{ key: Key }>(),
        layout: types<{ surface: Surface; moment: Moment; layout: Layout }>(),
        point: types<{ issue: string }>(),
        toggle: types<{ issue: string }>(),
        created: types<{ issue: string }>(),
        openMenu: types<{ query: string }>(),
        openCreate: types<void>(),
        openHelp: types<void>(),
        peek: types<void>(),
        peekAt: types<{ issue: string }>(),
        dismiss: types<void>(),
        close: types<void>(),
        clearSelection: types<void>(),
        laidOut: types<void>(),
      },
    },
    delays: { goPrefix: GO_PREFIX_MS },
  }).createMachine({
    id: "keys",
    context: {
      surface: "other",
      moment: "now",
      layout: [],
      cursor: undefined,
      selected: [],
      menuQuery: "",
    },
    type: "parallel",
    on: {
      layout: ({ context, event }, enq) => {
        enq.raise({ type: "laidOut" });
        return {
          context: {
            surface: event.surface,
            moment: event.moment,
            layout: event.layout,
            cursor: followed(context.layout, event.layout, context.cursor),
            selected:
              event.moment === "past"
                ? []
                : context.selected.filter((id) => contains(event.layout, id)),
          },
        };
      },
      point: ({ event }) => ({ context: { cursor: event.issue } }),
      toggle: ({ context, event }) =>
        context.moment === "now"
          ? { context: { cursor: event.issue, selected: toggled(context.selected, event.issue) } }
          : { context: { cursor: event.issue } },
      created: ({ event }) => ({ context: { cursor: event.issue, selected: [] } }),
      clearSelection: { context: { selected: [] } },
    },
    states: {
      overlay: {
        initial: "none",
        states: {
          none: {
            initial: "plain",
            states: {
              plain: {
                on: {
                  key: ({ context, event }, enq) => {
                    const onIssues = context.surface !== "other";
                    const changeable = onIssues && context.moment === "now";
                    switch (event.key) {
                      case "j":
                      case "k":
                        return onIssues
                          ? {
                              context: {
                                cursor: vertical(
                                  context.layout,
                                  context.cursor,
                                  event.key === "j" ? 1 : -1,
                                ),
                              },
                            }
                          : undefined;
                      case "h":
                      case "l":
                        return context.surface === "board"
                          ? {
                              context: {
                                cursor: horizontal(
                                  context.layout,
                                  context.cursor,
                                  event.key === "l" ? 1 : -1,
                                ),
                              },
                            }
                          : undefined;
                      case "H":
                      case "L": {
                        const issues = targets(context);
                        if (changeable && issues.length > 0) {
                          enq(effects.shift, issues, event.key === "L" ? 1 : -1);
                        }
                        return undefined;
                      }
                      case "x":
                        return changeable && context.cursor !== undefined
                          ? { context: { selected: toggled(context.selected, context.cursor) } }
                          : undefined;
                      case "space":
                        enq.raise({ type: "peek" });
                        return undefined;
                      case "escape":
                        enq.raise({ type: "dismiss" });
                        return undefined;
                      case "enter":
                        if (onIssues && context.cursor !== undefined) {
                          enq(effects.open, context.cursor);
                        }
                        return undefined;
                      case "g":
                        return { target: "go" };
                      case "c":
                        return context.moment === "now"
                          ? { target: "#keys.overlay.create" }
                          : undefined;
                      case "?":
                        return { target: "#keys.overlay.help" };
                      case "mod+k":
                        return { target: "#keys.overlay.menu", context: { menuQuery: "" } };
                      case "s":
                        return changeable
                          ? { target: "#keys.overlay.menu", context: { menuQuery: "Status " } }
                          : undefined;
                      case "p":
                        return changeable
                          ? { target: "#keys.overlay.menu", context: { menuQuery: "Priority " } }
                          : undefined;
                      case "a":
                        return changeable
                          ? { target: "#keys.overlay.menu", context: { menuQuery: "Assign " } }
                          : undefined;
                      case "mod+b":
                        if (onIssues) {
                          enq(effects.switchView);
                        }
                        return undefined;
                      case "/":
                        if (onIssues) {
                          enq(effects.focusSearch);
                        }
                        return undefined;
                      case "i":
                      case "m":
                        return undefined;
                      default: {
                        const unhandled: never = event.key;
                        return unhandled;
                      }
                    }
                  },
                },
              },
              go: {
                after: { goPrefix: { target: "plain" } },
                on: {
                  key: ({ event }, enq) => {
                    if (event.key === "i") {
                      enq(effects.goToWaiting);
                    } else if (event.key === "m") {
                      enq(effects.goToNow);
                    } else if (event.key === "h") {
                      enq(effects.goToHistory);
                    }
                    return { target: "plain" };
                  },
                },
              },
            },
            on: {
              openMenu: ({ event }) => ({ target: "menu", context: { menuQuery: event.query } }),
              openCreate: ({ context }) =>
                context.moment === "now" ? { target: "create" } : undefined,
              openHelp: { target: "help" },
            },
          },
          menu: {
            on: {
              key: ({ event }) =>
                event.key === "mod+k" || event.key === "escape" ? { target: "none" } : undefined,
              close: { target: "none" },
              laidOut: ({ context }) =>
                context.moment === "past" ? { target: "none" } : undefined,
              openCreate: ({ context }) =>
                context.moment === "now" ? { target: "create" } : undefined,
              openHelp: { target: "help" },
            },
          },
          help: {
            on: {
              key: ({ event }) =>
                event.key === "escape" || event.key === "?" ? { target: "none" } : undefined,
              close: { target: "none" },
            },
          },
          create: {
            on: {
              key: ({ event }) => (event.key === "escape" ? { target: "none" } : undefined),
              close: { target: "none" },
              laidOut: ({ context }) =>
                context.moment === "past" ? { target: "none" } : undefined,
            },
          },
        },
      },
      peek: {
        initial: "closed",
        states: {
          closed: {
            on: {
              peek: ({ context }) =>
                context.surface !== "other" &&
                context.moment === "now" &&
                context.cursor !== undefined
                  ? { target: "open" }
                  : undefined,
              dismiss: { context: { selected: [] } },
              peekAt: ({ context, event }) =>
                context.surface !== "other" &&
                context.moment === "now" &&
                contains(context.layout, event.issue)
                  ? { target: "open", context: { cursor: event.issue } }
                  : undefined,
            },
          },
          open: {
            on: {
              peekAt: ({ context, event }) =>
                contains(context.layout, event.issue)
                  ? { context: { cursor: event.issue } }
                  : undefined,
              peek: { target: "closed" },
              dismiss: { target: "closed" },
              laidOut: ({ context }) =>
                context.surface === "other" || context.moment === "past"
                  ? { target: "closed" }
                  : undefined,
            },
          },
        },
      },
    },
  });
}
