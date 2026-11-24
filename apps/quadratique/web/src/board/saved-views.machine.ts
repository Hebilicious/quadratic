import { Option, Schema } from "effect";
import { setup, types } from "xstate";

import { filterEntries, filterOf } from "../route.ts";
import { ALL, projectScope, sameScope, type Filter, type Scope } from "./view.ts";

export interface SavedView {
  readonly id: string;
  readonly name: string;
  readonly scope: Scope;
  readonly filter: Filter;
}

const StoredSchema = Schema.Struct({
  version: Schema.Literal(1),
  views: Schema.Array(
    Schema.Struct({
      id: Schema.String,
      name: Schema.String,
      project: Schema.optionalKey(Schema.String),
      filter: Schema.String,
    }),
  ),
});
type Stored = typeof StoredSchema.Type;

const decodeStored = Schema.decodeUnknownOption(StoredSchema);

export function storedOf(views: readonly SavedView[]): Stored {
  return {
    version: 1,
    views: views.map((view) => ({
      id: view.id,
      name: view.name,
      ...(view.scope.kind === "project" ? { project: view.scope.key } : {}),
      filter: new URLSearchParams(
        filterEntries(view.filter).map(([name, value]) => [name, value]),
      ).toString(),
    })),
  };
}

export function viewsOf(stored: unknown): readonly SavedView[] {
  const decoded = decodeStored(stored);
  if (Option.isNone(decoded)) {
    return [];
  }
  return decoded.value.views.map((view) => ({
    id: view.id,
    name: view.name,
    scope: view.project === undefined ? ALL : projectScope(view.project),
    filter: filterOf(new URLSearchParams(view.filter)),
  }));
}

const sameName = (left: string, right: string): boolean =>
  left.trim().toLowerCase() === right.trim().toLowerCase();

export function nameProblem(
  views: readonly SavedView[],
  name: string,
  id: string | undefined,
): string | undefined {
  if (name.trim().length === 0) {
    return "Give the view a name.";
  }
  const taken = views.find((view) => view.id !== id && sameName(view.name, name));
  return taken === undefined ? undefined : `A view is already named ${taken.name}.`;
}

const asSets = (filter: Filter): string =>
  JSON.stringify(
    filterEntries({
      ...filter,
      statuses: filter.statuses.toSorted(),
      categories: filter.categories.toSorted(),
      priorities: filter.priorities.toSorted(),
      labels: filter.labels.toSorted(),
    }),
  );

export function savedViewOf(
  views: readonly SavedView[],
  scope: Scope,
  filter: Filter,
): SavedView | undefined {
  const wanted = asSets(filter);
  return views.find((view) => sameScope(view.scope, scope) && asSets(view.filter) === wanted);
}

export interface SavedViewPorts {
  readonly read: () => unknown;
  readonly write: (stored: Stored) => void;
}

export function nextId(views: readonly SavedView[]): string {
  const numbers = views.map((view) => Number(/^v(\d+)$/u.exec(view.id)?.[1] ?? 0));
  return `v${String(Math.max(0, ...numbers) + 1)}`;
}

type WriteNews =
  | { type: "stored"; form: string; views: readonly SavedView[] }
  | { type: "notStored"; form: string };

export const refusedWrite = (
  snapshot: { readonly context: { readonly refused: readonly string[] } },
  form: string,
): boolean => snapshot.context.refused.includes(form);

export function savedViewsMachine(ports: SavedViewPorts) {
  const store = (
    self: { send: (event: WriteNews) => void },
    form: string,
    views: readonly SavedView[],
  ) => {
    try {
      ports.write(storedOf(views));
    } catch {
      self.send({ type: "notStored", form });
      return;
    }
    self.send({ type: "stored", form, views });
  };
  return setup({
    schemas: {
      context: types<{
        readonly views: readonly SavedView[];
        readonly refused: readonly string[];
      }>(),
      events: {
        save: types<{ form: string; name: string; scope: Scope; filter: Filter }>(),
        rename: types<{ form: string; id: string; name: string }>(),
        remove: types<{ form: string; id: string }>(),
        loaded: types<{ stored: unknown }>(),
        stored: types<{ form: string; views: readonly SavedView[] }>(),
        notStored: types<{ form: string }>(),
        forget: types<{ form: string }>(),
      },
    },
  }).createMachine({
    id: "savedViews",
    context: () => ({ views: viewsOf(ports.read()), refused: [] }),
    on: {
      stored: ({ context, event }) => ({
        context: {
          views: event.views,
          refused: context.refused.filter((form) => form !== event.form),
        },
      }),
      notStored: ({ context, event }) =>
        context.refused.includes(event.form)
          ? undefined
          : { context: { refused: [...context.refused, event.form] } },
      forget: ({ context, event }) => ({
        context: { refused: context.refused.filter((form) => form !== event.form) },
      }),
      save: ({ context, event, self }, enq) => {
        const name = event.name.trim();
        if (name.length === 0) {
          return undefined;
        }
        const named = context.views.find((view) => sameName(view.name, name));
        const views =
          named === undefined
            ? [
                ...context.views,
                { id: nextId(context.views), name, scope: event.scope, filter: event.filter },
              ]
            : context.views.map((view) =>
                view.id === named.id ? { ...view, scope: event.scope, filter: event.filter } : view,
              );
        enq(store, self, event.form, views);
        return undefined;
      },
      rename: ({ context, event, self }, enq) => {
        if (nameProblem(context.views, event.name, event.id) !== undefined) {
          return undefined;
        }
        const views = context.views.map((view) =>
          view.id === event.id ? { ...view, name: event.name.trim() } : view,
        );
        enq(store, self, event.form, views);
        return undefined;
      },
      remove: ({ context, event, self }, enq) => {
        const views = context.views.filter((view) => view.id !== event.id);
        enq(store, self, event.form, views);
        return undefined;
      },
      loaded: ({ event }) => ({ context: { views: viewsOf(event.stored) } }),
    },
  });
}

export type SavedViewsMachine = ReturnType<typeof savedViewsMachine>;
