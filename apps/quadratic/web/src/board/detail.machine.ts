import { createAsyncLogic, setup, types, type ActorRefFrom } from "xstate";

import { Refused, type Quadratic } from "../core/quadratic.ts";
import type { HistoryChange, HistoryPage, Issue, IssueSummary } from "./model.ts";

interface DetailContext {
  readonly id: string;
  readonly issue: Issue | undefined;
  readonly history: readonly HistoryChange[];
  readonly historyCursor: string | undefined;
}

export function concerns(
  issue: Issue | undefined,
  id: string,
  changed: readonly IssueSummary[],
): boolean {
  const named = new Set([
    id,
    ...(issue?.relations.map((relation) => relation.issue) ?? []),
    ...(issue?.children ?? []),
    ...(issue?.parent === undefined ? [] : [issue.parent]),
  ]);
  return changed.some((each) => named.has(each.id) || each.parent === id);
}

const isNotFound = (error: unknown): boolean =>
  error instanceof Refused &&
  typeof error.failure === "object" &&
  error.failure !== null &&
  "_tag" in error.failure &&
  error.failure._tag === "NotFound";

export function detailMachine(quadratic: Pick<Quadratic, "issue" | "history">) {
  const load = createAsyncLogic<Issue, { readonly id: string }>({
    run: async ({ input, signal }) => await quadratic.issue(input.id, signal),
  });
  const loadHistory = createAsyncLogic<
    HistoryPage,
    { readonly id: string; readonly cursor: string | undefined }
  >({
    run: async ({ input, signal }) =>
      await quadratic.history({ issue: input.id, cursor: input.cursor }, signal),
  });

  return setup({
    schemas: {
      context: types<DetailContext>(),
      input: types<{ readonly id: string }>(),
      events: {
        changed: types<{ issues: readonly IssueSummary[] }>(),
        reload: types<void>(),
        retry: types<void>(),
        showHistory: types<void>(),
        moreHistory: types<void>(),
      },
    },
    actors: { load, loadHistory },
  }).createMachine({
    id: "detail",
    context: ({ input }) => ({
      id: input.id,
      issue: undefined,
      history: [],
      historyCursor: undefined,
    }),
    type: "parallel",
    states: {
      issue: {
        initial: "loading",
        states: {
          loading: {
            invoke: {
              src: "load",
              input: ({ context }) => ({ id: context.id }),
              onDone: ({ event }) => ({ target: "ready", context: { issue: event.output } }),
              onError: ({ event }) => ({ target: isNotFound(event.error) ? "missing" : "failed" }),
            },
            on: {
              changed: { target: "loading", reenter: true },
              reload: { target: "loading", reenter: true },
            },
          },
          ready: {
            initial: "current",
            states: {
              current: {},
              refreshing: {
                invoke: {
                  src: "load",
                  input: ({ context }) => ({ id: context.id }),
                  onDone: ({ event }) => ({ target: "current", context: { issue: event.output } }),
                  onError: { target: "outdated" },
                },
              },
              outdated: { on: { retry: { target: "refreshing" } } },
            },
            on: {
              changed: ({ context, event }) =>
                concerns(context.issue, context.id, event.issues)
                  ? { target: ".refreshing", reenter: true }
                  : undefined,
              reload: { target: ".refreshing", reenter: true },
            },
          },
          failed: { on: { retry: { target: "loading" }, reload: { target: "loading" } } },
          missing: {},
        },
      },
      history: {
        initial: "hidden",
        states: {
          hidden: { on: { showHistory: { target: "loading" } } },
          loading: {
            invoke: {
              src: "loadHistory",
              input: ({ context }) => ({ id: context.id, cursor: undefined }),
              onDone: ({ event }) => ({
                target: "shown",
                context: { history: event.output.items, historyCursor: event.output.nextCursor },
              }),
              onError: { target: "failed" },
            },
            on: {
              changed: ({ context, event }) =>
                event.issues.some((each) => each.id === context.id)
                  ? { target: "loading", reenter: true }
                  : undefined,
              reload: { target: "loading", reenter: true },
            },
          },
          shown: {
            initial: "idle",
            states: {
              idle: {
                on: {
                  moreHistory: ({ context }) =>
                    context.historyCursor === undefined ? undefined : { target: "more" },
                },
              },
              more: {
                invoke: {
                  src: "loadHistory",
                  input: ({ context }) => ({ id: context.id, cursor: context.historyCursor }),
                  onDone: ({ context, event }) => ({
                    target: "idle",
                    context: {
                      history: [...context.history, ...event.output.items],
                      historyCursor: event.output.nextCursor,
                    },
                  }),
                  onError: { target: "idle" },
                },
              },
            },
            on: {
              changed: ({ context, event }) =>
                event.issues.some((each) => each.id === context.id)
                  ? { target: "loading" }
                  : undefined,
              reload: { target: "loading" },
            },
          },
          failed: { on: { showHistory: { target: "loading" } } },
        },
      },
    },
  });
}

export type DetailActor = ActorRefFrom<ReturnType<typeof detailMachine>>;
