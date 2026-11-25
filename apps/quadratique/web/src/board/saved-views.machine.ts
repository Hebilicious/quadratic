import { createCallbackLogic, setup, types } from "xstate";

import { filterEntries, filterOf } from "../route.ts";
import {
  batches,
  planLegacy,
  withSource,
  type Kept,
  type OldView,
  type Shortened,
} from "./legacy-views.ts";
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

/**
 * Every saved view at one revision of the list, as a list answer, a write answer or the live channel gives it. The
 * revision only grows, so of two snapshots the page keeps the one with the higher revision, whatever order they come in.
 */
export interface Snapshot {
  readonly revision: number;
  readonly items: readonly StoredView[];
}

export interface Imported extends Snapshot {
  /** The old ids that were not imported because the server holds the most views. */
  readonly full: readonly string[];
}

export interface SavedViewPorts {
  readonly list: (signal: AbortSignal) => Promise<Snapshot>;
  /** Without `replace`, creates; with it, replaces the filters of that view, which must still have `name`. */
  readonly save: (view: NewView & { readonly replace?: string }) => Promise<Snapshot>;
  readonly rename: (id: string, name: string) => Promise<Snapshot>;
  readonly remove: (id: string) => Promise<Snapshot>;
  readonly import: (source: string, views: readonly OldView[]) => Promise<Imported>;
  /** Resolves once the subscription is open; each item is the snapshot after a change. */
  readonly changes: (signal: AbortSignal) => Promise<AsyncIterable<Snapshot>>;
  /** The value under the key where this browser kept views before the server kept them. */
  readonly legacy: {
    readonly read: () => string | null;
    readonly write: (value: string) => void;
    /** Removes the value only when it is still exactly `value`. */
    readonly removeIf: (value: string) => void;
  };
  /** A new id for this browser's old views. */
  readonly newSource: () => string;
}

/** Why a write was not kept: a failure the server named, or no answer. */
export type Problem = "NameTaken" | "Renamed" | "NotFound" | "TooManyViews" | "unavailable";

const NAMED_PROBLEMS: readonly Problem[] = ["NameTaken", "Renamed", "NotFound", "TooManyViews"];

export function problemOf(error: unknown): Problem {
  const tag =
    typeof error === "object" &&
    error !== null &&
    "failure" in error &&
    typeof error.failure === "object" &&
    error.failure !== null &&
    "_tag" in error.failure
      ? error.failure._tag
      : undefined;
  return NAMED_PROBLEMS.find((problem) => problem === tag) ?? "unavailable";
}

/** What the import of this browser's old views did, for the page to say. */
export interface ImportReport {
  readonly shortened: readonly Shortened[];
  readonly kept: readonly Kept[];
  /** The import stopped because the server did not answer; the next page goes on from where it stopped. */
  readonly interrupted: boolean;
}

const NO_REPORT: ImportReport = { shortened: [], kept: [], interrupted: false };

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

interface Writes {
  readonly pending: readonly string[];
  readonly refused: Readonly<Record<string, Problem>>;
}

/** Where the last write of a form is: none or kept (`idle`), sent (`pending`) or not kept. */
export function writeState(snapshot: { readonly context: Writes }, form: string): WriteState {
  if (snapshot.context.pending.includes(form)) {
    return "pending";
  }
  return form in snapshot.context.refused ? "refused" : "idle";
}

/** Why the last write of a form was not kept, if it was not. */
export function refusalOf(
  snapshot: { readonly context: Writes },
  form: string,
): Problem | undefined {
  return snapshot.context.refused[form];
}

const MAX_RETRY_MS = 10_000;

type Write =
  | { readonly kind: "save"; readonly view: NewView & { readonly replace?: string } }
  | { readonly kind: "rename"; readonly id: string; readonly name: string }
  | { readonly kind: "remove"; readonly id: string };

type WriteNews =
  | { type: "stored"; form: string; snapshot: Snapshot }
  | { type: "notStored"; form: string; problem: Problem };

const without = (
  refused: Readonly<Record<string, Problem>>,
  form: string,
): Readonly<Record<string, Problem>> =>
  Object.fromEntries(Object.entries(refused).filter(([each]) => each !== form));

export function savedViewsMachine(ports: SavedViewPorts) {
  const send = (write: Write): Promise<Snapshot> => {
    switch (write.kind) {
      case "save":
        return ports.save(write.view);
      case "rename":
        return ports.rename(write.id, write.name);
      case "remove":
        return ports.remove(write.id);
    }
  };
  const written = (self: { send: (event: WriteNews) => void }, form: string, write: Write) => {
    send(write).then(
      (snapshot) => {
        self.send({ type: "stored", form, snapshot });
      },
      (error: unknown) => {
        self.send({ type: "notStored", form, problem: problemOf(error) });
      },
    );
  };

  /**
   * Imports the views this browser kept, in bounded calls. The server keeps a receipt per old view, so an import that
   * stops goes on at the next page without making any view twice. The browser forgets its views only once every one is
   * in the server, and only when its value is still the one imported.
   */
  const importLegacy = createCallbackLogic<{ type: "never" }>(({ sendBack }) => {
    const done = (report: ImportReport) => {
      sendBack({ type: "imported", report });
    };
    const run = async (): Promise<ImportReport> => {
      const plan = planLegacy(ports.legacy.read());
      if (plan.kind !== "import") {
        return NO_REPORT;
      }
      let raw = plan.raw;
      let source = plan.source;
      if (source === undefined) {
        source = ports.newSource();
        raw = withSource(raw, source);
        ports.legacy.write(raw);
      }
      const full = new Set<string>();
      for (const batch of batches(plan.rows)) {
        const answer = await ports.import(source, batch);
        for (const old of answer.full) {
          full.add(old);
        }
        sendBack({ type: "snapshot", snapshot: answer });
      }
      const kept = [
        ...plan.kept,
        ...plan.rows
          .filter((row) => full.has(row.old))
          .map((row) => ({ name: row.name, reason: "the server holds the most views, 100" })),
      ];
      if (kept.length === 0) {
        ports.legacy.removeIf(raw);
      }
      return { shortened: plan.shortened, kept, interrupted: false };
    };
    run().then(done, () => {
      done({ ...NO_REPORT, interrupted: true });
    });
  });

  /**
   * Opens the live channel, then reads the list. The views come from both; the revision decides which is newer. A list
   * that fails, or a channel that ends, ends this actor: the machine reconnects and reads the list again.
   */
  const follow = createCallbackLogic<{ type: "never" }>(({ sendBack }) => {
    const abort = new AbortController();
    const run = async () => {
      const changes = await ports.changes(abort.signal);
      sendBack({ type: "opened" });
      const listed = ports.list(abort.signal).then((snapshot) => {
        sendBack({ type: "listed", snapshot });
      });
      const forwarded = (async () => {
        for await (const snapshot of changes) {
          sendBack({ type: "snapshot", snapshot });
        }
      })();
      await Promise.all([listed, forwarded]);
    };
    const lost = () => {
      if (!abort.signal.aborted) {
        abort.abort();
        sendBack({ type: "lost" });
      }
    };
    void run().then(lost, lost);
    return () => {
      abort.abort();
    };
  });

  /** Keeps the snapshot when it is newer than the views shown. */
  const newer = (context: { readonly revision: number }, snapshot: Snapshot) =>
    snapshot.revision > context.revision
      ? { revision: snapshot.revision, views: snapshot.items.map(viewOf) }
      : {};

  return setup({
    schemas: {
      context: types<{
        readonly views: readonly SavedView[];
        /** The revision of the views shown; -1 before the first snapshot. */
        readonly revision: number;
        readonly pending: readonly string[];
        readonly refused: Readonly<Record<string, Problem>>;
        readonly attempts: number;
        readonly report: ImportReport;
      }>(),
      events: {
        save: types<{
          form: string;
          name: string;
          scope: Scope;
          filter: Filter;
          replace: string | undefined;
        }>(),
        rename: types<{ form: string; id: string; name: string }>(),
        remove: types<{ form: string; id: string }>(),
        imported: types<{ report: ImportReport }>(),
        opened: types<void>(),
        listed: types<{ snapshot: Snapshot }>(),
        snapshot: types<{ snapshot: Snapshot }>(),
        lost: types<void>(),
        stored: types<{ form: string; snapshot: Snapshot }>(),
        notStored: types<{ form: string; problem: Problem }>(),
        forget: types<{ form: string }>(),
      },
    },
    actors: { importLegacy, follow },
    delays: {
      retry: ({ context }) => Math.min(500 * 2 ** context.attempts, MAX_RETRY_MS),
    },
  }).createMachine({
    id: "savedViews",
    context: {
      views: [],
      revision: -1,
      pending: [],
      refused: {},
      attempts: 0,
      report: NO_REPORT,
    },
    initial: "importing",
    on: {
      save: ({ context, event, self }, enq) => {
        const name = event.name.trim();
        if (name.length === 0) {
          return undefined;
        }
        const view = {
          name,
          ...(event.scope.kind === "project" ? { project: event.scope.key } : {}),
          filter: storedFilter(event.filter),
          ...(event.replace === undefined ? {} : { replace: event.replace }),
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
          refused: without(context.refused, event.form),
          ...newer(context, event.snapshot),
        },
      }),
      notStored: ({ context, event }) => ({
        context: {
          pending: context.pending.filter((form) => form !== event.form),
          refused: { ...context.refused, [event.form]: event.problem },
        },
      }),
      forget: ({ context, event }) => ({
        context: { refused: without(context.refused, event.form) },
      }),
      snapshot: ({ context, event }) => ({ context: newer(context, event.snapshot) }),
    },
    states: {
      importing: {
        invoke: { src: "importLegacy" },
        on: {
          imported: ({ event }) => ({ target: "following", context: { report: event.report } }),
        },
      },
      following: {
        invoke: { src: "follow" },
        initial: "connecting",
        states: {
          connecting: {
            on: { opened: { target: "listing" } },
          },
          listing: {
            on: {
              listed: ({ context, event }) => ({
                target: "live",
                context: { attempts: 0, ...newer(context, event.snapshot) },
              }),
            },
          },
          live: {},
        },
        on: {
          lost: ({ context }) => ({
            target: "reconnecting",
            context: { attempts: context.attempts + 1 },
          }),
        },
      },
      reconnecting: {
        after: { retry: { target: "following" } },
      },
    },
  });
}

function sent(context: Writes, form: string): Writes {
  return {
    pending: context.pending.includes(form) ? context.pending : [...context.pending, form],
    refused: without(context.refused, form),
  };
}

/** True once the page shows the server's list: the live channel is open and its first list is in. */
export const viewsReady = (snapshot: { matches: (state: { following: "live" }) => boolean }) =>
  snapshot.matches({ following: "live" });

export type SavedViewsMachine = ReturnType<typeof savedViewsMachine>;
