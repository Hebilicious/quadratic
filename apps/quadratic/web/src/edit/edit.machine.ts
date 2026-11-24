import { createAsyncLogic, setup, types } from "xstate";

import { Refused } from "../core/quadratic.ts";

interface EditContext {
  readonly base: number;
  readonly error: unknown;
}

const isStale = (error: unknown): boolean =>
  error instanceof Refused &&
  typeof error.failure === "object" &&
  error.failure !== null &&
  "_tag" in error.failure &&
  error.failure._tag === "Stale";

export function editMachine(ports: {
  readonly run: (base: number, signal: AbortSignal) => Promise<unknown>;
  readonly revision: () => number;
  readonly refresh: () => void;
  readonly saved?: () => void;
  readonly landed?: () => boolean;
}) {
  const landed = (): boolean => ports.landed?.() === true;
  const save = createAsyncLogic<unknown, { readonly base: number }>({
    run: async ({ input, signal }) => await ports.run(input.base, signal),
  });

  return setup({
    schemas: {
      context: types<EditContext>(),
      events: {
        start: types<void>(),
        resume: types<{ base: number }>(),
        revised: types<{ revision: number }>(),
        submit: types<void>(),
        save: types<void>(),
        cancel: types<void>(),
      },
    },
    actors: { save },
  }).createMachine({
    id: "edit",
    context: { base: 0, error: undefined },
    initial: "idle",
    states: {
      idle: {
        on: {
          start: () => ({ target: "editing", context: { base: ports.revision() } }),
          resume: ({ event }, enq) => {
            if (landed()) {
              if (ports.saved !== undefined) {
                enq(ports.saved);
              }
              return undefined;
            }
            return { target: "editing", context: { base: event.base } };
          },
          submit: () => ({ target: "saving", context: { base: ports.revision() } }),
        },
      },
      editing: {
        on: {
          save: { target: "saving" },
          cancel: { target: "idle" },
        },
      },
      saving: {
        invoke: {
          src: "save",
          input: ({ context }) => ({ base: context.base }),
          onDone: (_, enq) => {
            if (ports.saved !== undefined) {
              enq(ports.saved);
            }
            return { target: "idle" };
          },
          onError: ({ context, event }, enq) => {
            if (isStale(event.error)) {
              enq(ports.refresh);
              return {
                target: ports.revision() > context.base ? "stale.refreshed" : "stale.refreshing",
                context: { error: event.error },
              };
            }
            return { target: "refused", context: { error: event.error } };
          },
        },
      },
      refused: {
        on: {
          start: { target: "editing" },
          submit: () => ({ target: "saving", context: { base: ports.revision() } }),
          save: { target: "saving" },
          cancel: { target: "idle" },
        },
      },
      stale: {
        initial: "refreshing",
        states: {
          refreshing: {
            on: {
              revised: ({ context, event }) =>
                event.revision > context.base ? { target: "refreshed" } : undefined,
            },
          },
          refreshed: {
            always: (_, enq) => {
              if (!landed()) {
                return undefined;
              }
              if (ports.saved !== undefined) {
                enq(ports.saved);
              }
              return { target: "#edit.idle", context: { error: undefined } };
            },
            on: {
              submit: () => ({ target: "#edit.saving", context: { base: ports.revision() } }),
              save: () => ({ target: "#edit.saving", context: { base: ports.revision() } }),
            },
          },
        },
        on: {
          cancel: { target: "idle" },
        },
      },
    },
  });
}

export type EditPhase = "idle" | "editing" | "saving" | "refused" | "stale";

export function editPhase(snapshot: { readonly value: unknown }): EditPhase {
  const { value } = snapshot;
  if (value === "idle" || value === "editing" || value === "saving" || value === "refused") {
    return value;
  }
  if (typeof value === "object" && value !== null && "stale" in value) {
    return "stale";
  }
  throw new Error(`The edit is in no known state: ${JSON.stringify(value)}`);
}
