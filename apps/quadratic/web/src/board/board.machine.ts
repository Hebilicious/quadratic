import { createAsyncLogic, setup, types, type ActorRefFrom } from "xstate";

import type { Head, Quadratic } from "../core/quadratic.ts";
import { connectionMachine } from "./connection.machine.ts";
import { detailMachine } from "./detail.machine.ts";
import { applied, issuesById, type Issues, type Update } from "./issues.ts";
import { changeKey, moreChangesThanOnePageHolds, prepended } from "./history.ts";
import type { HistoryChange, HistoryPage, IssueChanged, IssueSummary, Project } from "./model.ts";
import { appliedToNow, UNRESOLVED } from "./now.ts";
import { inScope, sameScope, type Scope } from "./view.ts";

interface BoardContext {
  readonly projects: readonly Project[];
  readonly scope: Scope | undefined;
  readonly at: string | undefined;
  readonly issues: Issues;
  readonly issuesSince: string | undefined;
  readonly moment: Head | undefined;
  readonly issuesError: unknown;
  readonly milestones: Readonly<Record<string, string | null>>;
  readonly milestonesStale: readonly string[];
  readonly milestonesAsked: readonly string[];
  readonly pending: readonly Update[];
  readonly now: Issues;
  readonly nowSince: string | undefined;
  readonly nowPending: readonly Update[];
  readonly open: string | undefined;
  readonly historyScope: Scope | undefined;
  readonly changes: readonly HistoryChange[];
  readonly changesCursor: string | undefined;
}

const changedIn = (payload: IssueChanged): readonly IssueSummary[] =>
  payload.issues.map((changed) => changed.issue);

const changeOf = (payload: IssueChanged): Update => ({
  from: "change",
  at: payload.at,
  issues: changedIn(payload),
});

const answerOf = (issues: readonly IssueSummary[]): Update => ({ from: "answer", issues });

interface Snapshot {
  readonly since: string | undefined;
  readonly items: readonly IssueSummary[];
}

interface PastSnapshot {
  readonly moment: Head | undefined;
  readonly items: readonly IssueSummary[];
}

const MILESTONE_READS = 8;

const NO_MILESTONES = { milestones: {}, milestonesStale: [], milestonesAsked: [] };

const staleInScope = (context: BoardContext): readonly string[] =>
  context.milestonesStale.filter((id) => id in context.issues);

const unread = (context: BoardContext): readonly string[] =>
  Object.keys(context.issues).filter((id) => !(id in context.milestones));

const milestoneRead = (context: BoardContext) => ({
  milestonesAsked: [...new Set([...unread(context), ...staleInScope(context)])],
  milestonesStale: [],
});

const keepIn =
  (scope: Scope | undefined) =>
  (issue: IssueSummary): boolean =>
    scope !== undefined && inScope(scope, issue);

const concernsScope = (scope: Scope | undefined, payload: IssueChanged): boolean =>
  scope !== undefined &&
  (scope.kind === "all" ||
    payload.projects.includes(scope.key) ||
    payload.issues.some((changed) => changed.issue.project === scope.key));

const rereadsOnResync = (context: BoardContext): boolean =>
  context.scope !== undefined && context.at === undefined;

const projectOf = (scope: Scope): { readonly project?: string } =>
  scope.kind === "project" ? { project: scope.key } : {};

export function boardMachine(
  quadratic: Pick<Quadratic, "projects" | "head" | "matching" | "changes" | "issue" | "history">,
) {
  const loadProjects = createAsyncLogic<readonly Project[], void>({
    run: async ({ signal }) => await quadratic.projects(signal),
  });
  const loadIssues = createAsyncLogic<Snapshot, { readonly scope: Scope }>({
    run: async ({ input, signal }) => {
      const head = await quadratic.head(signal);
      const asOf = head === undefined ? {} : { asOf: head.commit };
      return {
        since: head?.at,
        items: await quadratic.matching({ ...projectOf(input.scope), ...asOf }, signal),
      };
    },
  });
  const loadPast = createAsyncLogic<PastSnapshot, { readonly scope: Scope; readonly at: string }>({
    run: async ({ input, signal }) => {
      const moment = await quadratic.head(signal, input.at);
      return {
        moment,
        items:
          moment === undefined
            ? []
            : await quadratic.matching({ ...projectOf(input.scope), asOf: moment.commit }, signal),
      };
    },
  });
  const loadMilestones = createAsyncLogic<
    Readonly<Record<string, string | null>>,
    { readonly ids: readonly string[]; readonly asOf: string | undefined }
  >({
    run: async ({ input, signal }) => {
      const read: Record<string, string | null> = {};
      const queue = [...input.ids];
      const worker = async () => {
        for (let id = queue.shift(); id !== undefined; id = queue.shift()) {
          const issue = await quadratic.issue(id, signal, input.asOf);
          read[id] = issue.milestone ?? null;
        }
      };
      await Promise.all(Array.from({ length: MILESTONE_READS }, worker));
      return read;
    },
  });
  const loadNow = createAsyncLogic<Snapshot, void>({
    run: async ({ signal }) => {
      const head = await quadratic.head(signal);
      const asOf = head === undefined ? {} : { asOf: head.commit };
      const lists = await Promise.all([
        quadratic.matching({ claim: "any", ...asOf }, signal),
        quadratic.matching({ assignee: { kind: "human" }, category: UNRESOLVED, ...asOf }, signal),
        quadratic.matching({ category: ["started"], ...asOf }, signal),
      ]);
      return { since: head?.at, items: lists.flat() };
    },
  });

  const loadChanges = createAsyncLogic<
    HistoryPage,
    { readonly scope: Scope; readonly cursor: string | undefined }
  >({
    run: async ({ input, signal }) =>
      await quadratic.history({ ...projectOf(input.scope), cursor: input.cursor }, signal),
  });

  const detail = detailMachine(quadratic);
  const connection = connectionMachine(quadratic);
  return setup({
    schemas: {
      context: types<BoardContext>(),
      children: {
        connection: types<ActorRefFrom<typeof connection>>(),
        detail: types<ActorRefFrom<typeof detail>>(),
      },
      events: {
        select: types<{ scope: Scope | undefined; at: string | undefined }>(),
        milestones: types<{ wanted: boolean }>(),
        issuesRead: types<void>(),
        open: types<{ issue: string | undefined }>(),
        changed: types<{ payload: IssueChanged }>(),
        settled: types<{ issues: readonly IssueSummary[] }>(),
        resync: types<void>(),
        retry: types<void>(),
        history: types<{ scope: Scope | undefined }>(),
        moreChanges: types<void>(),
        replaced: types<void>(),
      },
    },
    actors: {
      connection,
      detail,
      loadProjects,
      loadIssues,
      loadPast,
      loadMilestones,
      loadNow,
      loadChanges,
    },
  }).createMachine({
    id: "board",
    context: {
      projects: [],
      scope: undefined,
      at: undefined,
      issues: {},
      issuesSince: undefined,
      moment: undefined,
      issuesError: undefined,
      ...NO_MILESTONES,
      pending: [],
      now: {},
      nowSince: undefined,
      nowPending: [],
      open: undefined,
      historyScope: undefined,
      changes: [],
      changesCursor: undefined,
    },
    invoke: { id: "connection", src: "connection" },
    type: "parallel",
    states: {
      projects: {
        initial: "waiting",
        states: {
          waiting: {},
          loading: {
            invoke: {
              src: "loadProjects",
              onDone: ({ event }) => ({ target: "ready", context: { projects: event.output } }),
              onError: { target: "failed" },
            },
          },
          ready: {},
          failed: { on: { retry: { target: "loading" } } },
        },
        on: {
          resync: { target: ".loading", reenter: true },
          changed: ({ event }) =>
            event.payload.projects.length > 0 ? { target: ".loading", reenter: true } : undefined,
        },
      },
      issues: {
        initial: "unselected",
        states: {
          unselected: {},
          loading: {
            invoke: {
              src: "loadIssues",
              input: ({ context }) => ({ scope: context.scope ?? { kind: "all" } }),
              onDone: ({ context, event }, enq) => {
                const options = { since: event.output.since, keep: keepIn(context.scope) };
                enq.raise({ type: "issuesRead" });
                return {
                  target: "ready",
                  context: {
                    issues: context.pending.reduce(
                      (issues, update) => applied(issues, update, options),
                      issuesById(event.output.items),
                    ),
                    issuesSince: event.output.since,
                    pending: [],
                  },
                };
              },
              onError: ({ event }) => ({
                target: "failed",
                context: { pending: [], issuesError: event.error },
              }),
            },
            on: {
              changed: ({ context, event }) => ({
                context: { pending: [...context.pending, changeOf(event.payload)] },
              }),
              settled: ({ context, event }) => ({
                context: { pending: [...context.pending, answerOf(event.issues)] },
              }),
            },
          },
          ready: {
            on: {
              changed: ({ context, event }) => ({
                context: {
                  issues: applied(context.issues, changeOf(event.payload), {
                    since: context.issuesSince,
                    keep: keepIn(context.scope),
                  }),
                },
              }),
              settled: ({ context, event }) => ({
                context: {
                  issues: applied(context.issues, answerOf(event.issues), {
                    since: context.issuesSince,
                    keep: keepIn(context.scope),
                  }),
                },
              }),
            },
          },
          loadingPast: {
            invoke: {
              src: "loadPast",
              input: ({ context }) => ({
                scope: context.scope ?? { kind: "all" },
                at: context.at ?? "",
              }),
              onDone: ({ event }, enq) => {
                enq.raise({ type: "issuesRead" });
                return {
                  target: "past",
                  context: { issues: issuesById(event.output.items), moment: event.output.moment },
                };
              },
              onError: ({ event }) => ({ target: "failed", context: { issuesError: event.error } }),
            },
          },
          past: {},
          failed: {
            on: {
              retry: ({ context }) => ({
                target: context.at === undefined ? "loading" : "loadingPast",
                context: { issuesError: undefined },
              }),
            },
          },
        },
        on: {
          select: ({ context, event }) => {
            if (sameScope(event.scope, context.scope) && event.at === context.at) {
              return undefined;
            }
            const cleared = {
              scope: event.scope,
              at: event.at,
              issues: {},
              pending: [],
              moment: undefined,
              issuesError: undefined,
              ...NO_MILESTONES,
            };
            if (event.scope === undefined) {
              return { target: ".unselected", context: cleared };
            }
            return {
              target: event.at === undefined ? ".loading" : ".loadingPast",
              reenter: true,
              context: cleared,
            };
          },
          resync: ({ context }) =>
            rereadsOnResync(context)
              ? { target: ".loading", reenter: true, context: { pending: [] } }
              : undefined,
        },
      },
      milestones: {
        initial: "off",
        states: {
          off: {
            on: {
              milestones: ({ context, event }) =>
                event.wanted ? { target: "on", context: milestoneRead(context) } : undefined,
            },
          },
          on: {
            initial: "reading",
            states: {
              waiting: {},
              reading: {
                invoke: {
                  src: "loadMilestones",
                  input: ({ context }) => ({
                    ids: context.milestonesAsked,
                    asOf: context.moment?.commit ?? context.at,
                  }),
                  onDone: ({ context, event }) => ({
                    target: "read",
                    context: {
                      milestones: {
                        ...context.milestones,
                        ...Object.fromEntries(
                          Object.entries(event.output).filter(
                            ([id]) => !context.milestonesStale.includes(id),
                          ),
                        ),
                      },
                      milestonesStale: staleInScope(context),
                      milestonesAsked: [],
                    },
                  }),
                  onError: ({ context }) => ({
                    target: "failed",
                    context: {
                      milestonesStale: [...context.milestonesAsked, ...context.milestonesStale],
                      milestonesAsked: [],
                    },
                  }),
                },
              },
              read: {
                always: ({ context }) =>
                  staleInScope(context).length > 0 || unread(context).length > 0
                    ? { target: "reading", context: milestoneRead(context) }
                    : undefined,
              },
              failed: {
                on: {
                  retry: ({ context }) => ({ target: "reading", context: milestoneRead(context) }),
                },
              },
            },
            on: {
              milestones: ({ event }) =>
                event.wanted ? undefined : { target: "off", context: NO_MILESTONES },
              issuesRead: ({ context }) => ({
                target: ".reading",
                reenter: true,
                context: milestoneRead(context),
              }),
              select: ({ context, event }) =>
                sameScope(event.scope, context.scope) && event.at === context.at
                  ? undefined
                  : { target: ".waiting", context: NO_MILESTONES },
              resync: ({ context }) =>
                rereadsOnResync(context)
                  ? { target: ".waiting", context: NO_MILESTONES }
                  : undefined,
              changed: ({ context, event }) =>
                context.at === undefined
                  ? {
                      context: {
                        milestonesStale: [
                          ...context.milestonesStale,
                          ...changedIn(event.payload).map((issue) => issue.id),
                        ],
                      },
                    }
                  : undefined,
              settled: ({ context, event }) => ({
                context: {
                  milestonesStale: [
                    ...context.milestonesStale,
                    ...event.issues.map((issue) => issue.id),
                  ],
                },
              }),
            },
          },
        },
      },
      now: {
        initial: "waiting",
        states: {
          waiting: {},
          loading: {
            invoke: {
              src: "loadNow",
              onDone: ({ context, event }) => ({
                target: "ready",
                context: {
                  now: context.nowPending.reduce(
                    (now, update) => appliedToNow(now, update, event.output.since),
                    issuesById(event.output.items),
                  ),
                  nowSince: event.output.since,
                  nowPending: [],
                },
              }),
              onError: { target: "failed", context: { nowPending: [] } },
            },
            on: {
              changed: ({ context, event }) => ({
                context: { nowPending: [...context.nowPending, changeOf(event.payload)] },
              }),
              settled: ({ context, event }) => ({
                context: { nowPending: [...context.nowPending, answerOf(event.issues)] },
              }),
            },
          },
          ready: {
            on: {
              changed: ({ context, event }) => ({
                context: {
                  now: appliedToNow(context.now, changeOf(event.payload), context.nowSince),
                },
              }),
              settled: ({ context, event }) => ({
                context: {
                  now: appliedToNow(context.now, answerOf(event.issues), context.nowSince),
                },
              }),
            },
          },
          failed: { on: { retry: { target: "loading" } } },
        },
        on: {
          resync: { target: ".loading", reenter: true, context: { nowPending: [] } },
        },
      },
      history: {
        initial: "hidden",
        states: {
          hidden: {},
          loading: {
            invoke: {
              src: "loadChanges",
              input: ({ context }) => ({
                scope: context.historyScope ?? { kind: "all" },
                cursor: undefined,
              }),
              onDone: ({ event }) => ({
                target: "shown",
                context: { changes: event.output.items, changesCursor: event.output.nextCursor },
              }),
              onError: { target: "failed" },
            },
            on: {
              changed: ({ context, event }) =>
                concernsScope(context.historyScope, event.payload)
                  ? { target: "loading", reenter: true }
                  : undefined,
            },
          },
          shown: {
            type: "parallel",
            states: {
              older: {
                initial: "idle",
                states: {
                  idle: {
                    on: {
                      moreChanges: ({ context }) =>
                        context.changesCursor === undefined ? undefined : { target: "loading" },
                    },
                  },
                  loading: {
                    invoke: {
                      src: "loadChanges",
                      input: ({ context }) => ({
                        scope: context.historyScope ?? { kind: "all" },
                        cursor: context.changesCursor,
                      }),
                      onDone: ({ context, event }) => {
                        const known = new Set(context.changes.map(changeKey));
                        return {
                          target: "idle",
                          context: {
                            changes: [
                              ...context.changes,
                              ...event.output.items.filter(
                                (change) => !known.has(changeKey(change)),
                              ),
                            ],
                            changesCursor: event.output.nextCursor,
                          },
                        };
                      },
                      onError: { target: "failed" },
                    },
                  },
                  failed: {
                    on: { moreChanges: { target: "loading" } },
                  },
                },
                on: {
                  replaced: { target: ".idle" },
                },
              },
              newest: {
                initial: "idle",
                states: {
                  idle: {
                    on: {
                      changed: ({ context, event }) =>
                        concernsScope(context.historyScope, event.payload)
                          ? { target: "refreshing" }
                          : undefined,
                    },
                  },
                  refreshing: {
                    invoke: {
                      src: "loadChanges",
                      input: ({ context }) => ({
                        scope: context.historyScope ?? { kind: "all" },
                        cursor: undefined,
                      }),
                      onDone: ({ context, event }, enq) => {
                        if (moreChangesThanOnePageHolds(context, event.output)) {
                          enq.raise({ type: "replaced" });
                        }
                        return { target: "idle", context: prepended(context, event.output) };
                      },
                      onError: { target: "idle" },
                    },
                    on: {
                      changed: ({ context, event }) =>
                        concernsScope(context.historyScope, event.payload)
                          ? { target: "refreshing", reenter: true }
                          : undefined,
                    },
                  },
                },
              },
            },
          },
          failed: { on: { retry: { target: "loading" } } },
        },
        on: {
          history: ({ context, event }) => {
            if (sameScope(event.scope, context.historyScope)) {
              return undefined;
            }
            return event.scope === undefined
              ? {
                  target: ".hidden",
                  context: { historyScope: undefined, changes: [], changesCursor: undefined },
                }
              : {
                  target: ".loading",
                  reenter: true,
                  context: { historyScope: event.scope, changes: [], changesCursor: undefined },
                };
          },
          resync: ({ context }) =>
            context.historyScope === undefined ? undefined : { target: ".loading", reenter: true },
        },
      },
      detail: {
        initial: "closed",
        states: {
          closed: {},
          open: {
            invoke: {
              id: "detail",
              src: "detail",
              input: ({ context }) => ({ id: context.open ?? "" }),
            },
            on: {
              changed: ({ children, event }, enq) => {
                enq.sendTo(children.detail, { type: "changed", issues: changedIn(event.payload) });
              },
              settled: ({ children, event }, enq) => {
                enq.sendTo(children.detail, { type: "changed", issues: event.issues });
              },
              resync: ({ children }, enq) => {
                enq.sendTo(children.detail, { type: "reload" });
              },
            },
          },
        },
        on: {
          open: ({ context, event }) => {
            if (event.issue === context.open) {
              return undefined;
            }
            return event.issue === undefined
              ? { target: ".closed", context: { open: undefined } }
              : { target: ".open", reenter: true, context: { open: event.issue } };
          },
        },
      },
    },
  });
}

export type BoardMachine = ReturnType<typeof boardMachine>;
