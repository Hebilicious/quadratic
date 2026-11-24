import { createAsyncLogic, setup, types } from "xstate";

import type { Assignee, IssueSummary } from "../board/model.ts";
import type { NewIssue } from "../core/quadratique.ts";

export interface Draft {
  readonly project: string;
  readonly title: string;
  readonly description: string;
  readonly acceptance: string;
  readonly status: string;
  readonly priority: string;
  readonly labels: string;
  readonly assignee: string;
  readonly parent: string;
}

export type DraftField = Exclude<keyof Draft, "project">;

interface CreateContext {
  readonly draft: Draft;
  readonly parents: readonly IssueSummary[];
  readonly error: unknown;
}

export interface CreatePorts {
  readonly create: (draft: Draft, signal: AbortSignal) => Promise<IssueSummary>;
  readonly parents: (project: string, signal: AbortSignal) => Promise<readonly IssueSummary[]>;
  readonly created: (issue: IssueSummary) => void;
  readonly dismissed: () => void;
}

export const emptyDraft = (project: string): Draft => ({
  project,
  title: "",
  description: "",
  acceptance: "",
  status: "",
  priority: "0",
  labels: "",
  assignee: "",
  parent: "",
});

export const canSubmit = (draft: Draft): boolean =>
  draft.project !== "" && draft.title.trim() !== "" && draft.acceptance.trim() !== "";

export function createMachine(ports: CreatePorts) {
  const submit = createAsyncLogic<IssueSummary, { readonly draft: Draft }>({
    run: async ({ input, signal }) => await ports.create(input.draft, signal),
  });
  const loadParents = createAsyncLogic<readonly IssueSummary[], { readonly project: string }>({
    run: async ({ input, signal }) =>
      input.project === "" ? [] : await ports.parents(input.project, signal),
  });

  return setup({
    schemas: {
      context: types<CreateContext>(),
      input: types<{ readonly project: string }>(),
      events: {
        field: types<{ name: DraftField; value: string }>(),
        project: types<{ key: string }>(),
        submit: types<void>(),
        dismiss: types<void>(),
        parentsChanged: types<void>(),
      },
    },
    actors: { submit, loadParents },
  }).createMachine({
    id: "create",
    context: ({ input }) => ({ draft: emptyDraft(input.project), parents: [], error: undefined }),
    type: "parallel",
    states: {
      form: {
        initial: "editing",
        states: {
          editing: {
            on: {
              field: ({ context, event }) => ({
                context: { draft: { ...context.draft, [event.name]: event.value } },
              }),
              project: ({ context, event }, enq) => {
                if (event.key === context.draft.project) {
                  return undefined;
                }
                enq.raise({ type: "parentsChanged" });
                return {
                  context: {
                    draft: { ...context.draft, project: event.key, status: "", parent: "" },
                    parents: [],
                  },
                };
              },
              submit: ({ context }) =>
                canSubmit(context.draft) ? { target: "submitting" } : undefined,
              dismiss: (_, enq) => {
                enq(ports.dismissed);
              },
            },
          },
          submitting: {
            invoke: {
              src: "submit",
              input: ({ context }) => ({ draft: context.draft }),
              onDone: ({ event }, enq) => {
                enq(ports.created, event.output);
                return { target: "created" };
              },
              onError: ({ event }) => ({ target: "editing", context: { error: event.error } }),
            },
          },
          created: { type: "final" },
        },
      },
      parents: {
        initial: "loading",
        states: {
          loading: {
            invoke: {
              src: "loadParents",
              input: ({ context }) => ({ project: context.draft.project }),
              onDone: ({ event }) => ({ target: "ready", context: { parents: event.output } }),
              onError: { target: "failed" },
            },
          },
          ready: {},
          failed: {},
        },
        on: {
          parentsChanged: { target: ".loading", reenter: true },
        },
      },
    },
  });
}

export function newIssue(draft: Draft, labels: readonly string[]): NewIssue {
  const assignee: Assignee | undefined =
    draft.assignee === ""
      ? undefined
      : draft.assignee === "human"
        ? { kind: "human" }
        : { kind: "agent", role: draft.assignee.slice("role:".length) };
  return {
    project: draft.project,
    title: draft.title.trim(),
    acceptance: draft.acceptance.trim(),
    priority: Number(draft.priority),
    ...(draft.description.trim() === "" ? {} : { description: draft.description.trim() }),
    ...(draft.status === "" ? {} : { status: draft.status }),
    ...(labels.length === 0 ? {} : { labels }),
    ...(assignee === undefined ? {} : { assignee }),
    ...(draft.parent === "" ? {} : { parent: draft.parent }),
  };
}
