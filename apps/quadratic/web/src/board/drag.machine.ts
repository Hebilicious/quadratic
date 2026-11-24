import { setup, types } from "xstate";

interface DragContext {
  readonly issue: string | undefined;
  readonly over: string | undefined;
}

export function dragMachine(drop: (move: { issue: string; group: string }) => void) {
  return setup({
    schemas: {
      context: types<DragContext>(),
      events: {
        start: types<{ issue: string }>(),
        over: types<{ group: string }>(),
        drop: types<{ group: string }>(),
        end: types<void>(),
      },
    },
  }).createMachine({
    id: "drag",
    context: { issue: undefined, over: undefined },
    initial: "idle",
    states: {
      idle: {
        on: {
          start: ({ event }) => ({ target: "dragging", context: { issue: event.issue } }),
        },
      },
      dragging: {
        on: {
          over: ({ event }) => ({ context: { over: event.group } }),
          drop: ({ context, event }, enq) => {
            if (context.issue !== undefined) {
              enq(drop, { issue: context.issue, group: event.group });
            }
            return { target: "idle", context: { issue: undefined, over: undefined } };
          },
          end: { target: "idle", context: { issue: undefined, over: undefined } },
        },
      },
    },
  });
}
