import { Option, Schema } from "effect";
import { createCallbackLogic, setup, types } from "xstate";

import { filterEntries, filterOf } from "../route.ts";
import { ALL, projectScope, sameScope, type Filter, type Scope } from "./view.ts";

export interface SavedView {
  readonly id: string;
  readonly name: string;
  readonly scope: Scope;
  readonly filter: Filter;
}

/** A saved view as the server keeps it: its filters as the query of the page's address. */
export interface StoredView {
  readonly id: string;
  readonly name: string;
  readonly project?: string;
  readonly filter: string;
}

export type NewView = Omit<StoredView, "id">;

/** What the server answers to a write, and what the live channel sends: every saved view. */
export type Views = readonly StoredView[];

export interface SavedViewPorts {
  readonly list: (signal: AbortSignal) => Promise<Views>;
  readonly save: (view: NewView) => Promise<Views>;
  readonly rename: (id: string, name: string) => Promise<Views>;
  readonly remove: (id: string) => Promise<Views>;
  readonly import: (views: readonly NewView[]) => Promise<Views>;
  /** Resolves once the subscription is open; each item is every saved view after a change. */
  readonly changes: (signal: AbortSignal) => Promise<AsyncIterable<Views>>;
  /** The views that this browser kept before the server kept them. */
  readonly legacy: { readonly read: () => unknown; readonly clear: () => void };
}

const LegacySchema = Schema.Struct({
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

const decodeLegacy = Schema.decodeUnknownOption(LegacySchema);

/** The views that a browser kept under the old key, in their order; none for any other shape. */
export function legacyViews(stored: unknown): readonly NewView[] {
  const decoded = decodeLegacy(stored);
  if (Option.isNone(decoded)) {
    return [];
  }
  return decoded.value.views
    .filter((view) => view.name.trim().length > 0)
    .map((view) => ({
      name: view.name.trim().slice(0, 100),
      ...(view.project === undefined ? {} : { project: view.project }),
      filter: view.filter,
    }));
}

export const storedFilter = (filter: Filter): string =>
  new URLSearchParams(filterEntries(filter).map(([name, value]) => [name, value])).toString();

export function viewOf(stored: StoredView): SavedView {
  return {
    id: stored.id,
    name: stored.name,
    scope: stored.project === undefined ? ALL : projectScope(stored.project),
    filter: filterOf(new URLSearchParams(stored.filter)),
  };
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

export type WriteState = "idle" | "pending" | "refused";

/** Where the last write of a form is: none or stored (`idle`), sent (`pending`) or not stored. */
export function writeState(
  snapshot: {
    readonly context: { readonly pending: readonly string[]; readonly refused: readonly string[] };
  },
  form: string,
): WriteState {
  if (snapshot.context.pending.includes(form)) {
    return "pending";
  }
  return snapshot.context.refused.includes(form) ? "refused" : "idle";
}

const MAX_RETRY_MS = 10_000;

type Write =
  | { readonly kind: "save"; readonly view: NewView }
  | { readonly kind: "rename"; readonly id: string; readonly name: string }
  | { readonly kind: "remove"; readonly id: string };

export function savedViewsMachine(ports: SavedViewPorts) {
  const send = (write: Write): Promise<Views> => {
    switch (write.kind) {
      case "save":
        return ports.save(write.view);
      case "rename":
        return ports.rename(write.id, write.name);
      case "remove":
        return ports.remove(write.id);
    }
  };
  const written = (
    self: {
      send: (
        event: { type: "stored"; form: string; views: Views } | { type: "notStored"; form: string },
      ) => void;
    },
    form: string,
    write: Write,
  ) => {
    send(write).then(
      (views) => {
        self.send({ type: "stored", form, views });
      },
      () => {
        self.send({ type: "notStored", form });
      },
    );
  };

  /** Imports the views this browser kept, once: the browser forgets them only after the server kept them. */
  const importLegacy = createCallbackLogic<{ type: "never" }>(({ sendBack }) => {
    const views = (() => {
      const stored = ports.legacy.read();
      return stored === null || stored === undefined ? undefined : legacyViews(stored);
    })();
    const forget = () => {
      ports.legacy.clear();
      sendBack({ type: "imported" });
    };
    if (views === undefined) {
      sendBack({ type: "imported" });
    } else if (views.length === 0) {
      forget();
    } else {
      ports.import(views).then(forget, () => {
        sendBack({ type: "imported" });
      });
    }
  });

  const follow = createCallbackLogic<{ type: "never" }>(({ sendBack }) => {
    const abort = new AbortController();
    const forwardEveryChange = async () => {
      const changes = await ports.changes(abort.signal);
      sendBack({ type: "opened" });
      const listed = ports.list(abort.signal).then((views) => {
        sendBack({ type: "loaded", views });
      });
      for await (const views of changes) {
        sendBack({ type: "loaded", views });
      }
      await listed;
    };
    const lost = () => {
      if (!abort.signal.aborted) {
        sendBack({ type: "lost" });
      }
    };
    void forwardEveryChange().then(lost, lost);
    return () => {
      abort.abort();
    };
  });

  const writeEvents = {
    save: types<{ form: string; name: string; scope: Scope; filter: Filter }>(),
    rename: types<{ form: string; id: string; name: string }>(),
    remove: types<{ form: string; id: string }>(),
  };

  return setup({
    schemas: {
      context: types<{
        readonly views: readonly SavedView[];
        readonly pending: readonly string[];
        readonly refused: readonly string[];
        readonly attempts: number;
        /** While the live channel is open it brings every change, the page's own included, in order. */
        readonly live: boolean;
      }>(),
      events: {
        ...writeEvents,
        imported: types<void>(),
        opened: types<void>(),
        loaded: types<{ views: Views }>(),
        lost: types<void>(),
        stored: types<{ form: string; views: Views }>(),
        notStored: types<{ form: string }>(),
        forget: types<{ form: string }>(),
      },
    },
    actors: { importLegacy, follow },
    delays: {
      retry: ({ context }) => Math.min(500 * 2 ** context.attempts, MAX_RETRY_MS),
    },
  }).createMachine({
    id: "savedViews",
    context: { views: [], pending: [], refused: [], attempts: 0, live: false },
    initial: "importing",
    on: {
      save: ({ context, event, self }, enq) => {
        const name = event.name.trim();
        if (name.length === 0) {
          return undefined;
        }
        const view: NewView = {
          name,
          ...(event.scope.kind === "project" ? { project: event.scope.key } : {}),
          filter: storedFilter(event.filter),
        };
        enq(written, self, event.form, { kind: "save", view });
        return { context: sent(context, event.form) };
      },
      rename: ({ context, event, self }, enq) => {
        if (nameProblem(context.views, event.name, event.id) !== undefined) {
          return undefined;
        }
        enq(written, self, event.form, { kind: "rename", id: event.id, name: event.name.trim() });
        return { context: sent(context, event.form) };
      },
      remove: ({ context, event, self }, enq) => {
        enq(written, self, event.form, { kind: "remove", id: event.id });
        return { context: sent(context, event.form) };
      },
      stored: ({ context, event }) => ({
        context: {
          pending: context.pending.filter((form) => form !== event.form),
          refused: context.refused.filter((form) => form !== event.form),
          ...(context.live ? {} : { views: event.views.map(viewOf) }),
        },
      }),
      notStored: ({ context, event }) => ({
        context: {
          pending: context.pending.filter((form) => form !== event.form),
          refused: context.refused.includes(event.form)
            ? context.refused
            : [...context.refused, event.form],
        },
      }),
      forget: ({ context, event }) => ({
        context: { refused: context.refused.filter((form) => form !== event.form) },
      }),
      loaded: ({ event }) => ({ context: { views: event.views.map(viewOf) } }),
    },
    states: {
      importing: {
        invoke: { src: "importLegacy" },
        on: { imported: { target: "following" } },
      },
      following: {
        invoke: { src: "follow" },
        on: {
          opened: () => ({ context: { attempts: 0, live: true } }),
          lost: ({ context }) => ({
            target: "reconnecting",
            context: { attempts: context.attempts + 1, live: false },
          }),
        },
      },
      reconnecting: {
        after: { retry: { target: "following" } },
      },
    },
  });
}

function sent(
  context: { readonly pending: readonly string[]; readonly refused: readonly string[] },
  form: string,
) {
  return {
    pending: context.pending.includes(form) ? context.pending : [...context.pending, form],
    refused: context.refused.filter((each) => each !== form),
  };
}

export type SavedViewsMachine = ReturnType<typeof savedViewsMachine>;
