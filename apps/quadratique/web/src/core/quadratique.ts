import * as Handles from "@acid-cat/core-effect/Handles";
import type * as WebMcp from "@acid-cat/core-effect/WebMcp";
import type { Occurrence } from "@acid-cat/spec-ts";
import { Schema } from "effect";

import {
  IssueAnswerSchema,
  IssueChangedSchema,
  IssuePageSchema,
  IssueRelationsSchema,
  IssueSummaryAnswerSchema,
  HistoryPageSchema,
  ProjectListSchema,
  SavedViewsSchema,
  ViewsImportedSchema,
  type Assignee,
  type HistoryPage,
  type Issue,
  type IssueChanged,
  type IssueSummary,
  type Project,
  type Relation,
  type RelationKind,
} from "../board/model.ts";
import type { SavedViewPorts, Snapshot } from "../board/saved-views.machine.ts";
import { app, sharedCatalog } from "./catalog.ts";
import { serverLayer, type Access } from "./server.ts";

export interface Change {
  readonly cursor: string | undefined;
  readonly truncated: boolean;
  readonly payload: IssueChanged;
}

export type ClearableField = "parent" | "milestone" | "assignee" | "description";

export interface IssuePatch {
  readonly title?: string;
  readonly status?: string;
  readonly priority?: number;
  readonly labels?: readonly string[];
  readonly assignee?: Assignee;
  readonly parent?: string;
  readonly milestone?: string;
  readonly description?: string;
  readonly acceptance?: string;
  readonly notes?: string;
  readonly clear?: readonly ClearableField[];
}

export interface NewIssue extends Omit<IssuePatch, "clear"> {
  readonly project: string;
  readonly title: string;
  readonly acceptance: string;
}

export interface IssueFilter {
  readonly project?: string;
  readonly asOf?: string;
  readonly claim?: "any";
  readonly assignee?: Assignee;
  readonly category?: readonly IssueSummary["category"][];
}

export interface Quadratique {
  readonly projects: (signal: AbortSignal) => Promise<readonly Project[]>;
  readonly head: (signal: AbortSignal, asOf?: string) => Promise<Head | undefined>;
  readonly matching: (filter: IssueFilter, signal: AbortSignal) => Promise<readonly IssueSummary[]>;
  readonly issue: (id: string, signal: AbortSignal, asOf?: string) => Promise<Issue>;
  readonly history: (
    request: {
      readonly issue?: string;
      readonly project?: string;
      readonly cursor: string | undefined;
    },
    signal: AbortSignal,
  ) => Promise<HistoryPage>;
  readonly update: (
    request: { readonly issue: string; readonly revision: number; readonly patch: IssuePatch },
    signal: AbortSignal,
  ) => Promise<IssueSummary>;
  readonly create: (issue: NewIssue, signal: AbortSignal) => Promise<IssueSummary>;
  readonly link: (
    request: {
      readonly issue: string;
      readonly kind: RelationKind;
      readonly target: string;
      readonly remove: boolean;
    },
    signal: AbortSignal,
  ) => Promise<readonly Relation[]>;
  readonly release: (issue: string, signal: AbortSignal) => Promise<IssueSummary>;
  readonly finish: (
    request: { readonly issue: string; readonly evidence: string },
    signal: AbortSignal,
  ) => Promise<Issue>;
  readonly changes: (
    cursor: string | undefined,
    signal: AbortSignal,
  ) => Promise<AsyncIterable<Change>>;
  readonly views: Omit<SavedViewPorts, "legacy" | "newSource">;
  readonly dispose: () => Promise<void>;
}

export interface Head {
  readonly commit: string;
  readonly at: string;
}

export class PastMoment extends Error {
  constructor(id: string) {
    super(`${id} was not sent: the page shows a past moment`);
  }
}

const isChanging = (id: string): boolean => sharedCatalog.capabilities[id]?.readOnly !== true;

export class Refused extends Error {
  readonly failure: unknown;
  constructor(id: string, failure: unknown) {
    super(`${id} was refused`);
    this.failure = failure;
  }
}

const decodePage = Schema.decodeUnknownSync(IssuePageSchema);
const decodeProjects = Schema.decodeUnknownSync(ProjectListSchema);
const decodeChange = Schema.decodeUnknownSync(IssueChangedSchema);
const decodeIssue = Schema.decodeUnknownSync(IssueAnswerSchema);
const decodeSummary = Schema.decodeUnknownSync(IssueSummaryAnswerSchema);
const decodeRelations = Schema.decodeUnknownSync(IssueRelationsSchema);
const decodeHistory = Schema.decodeUnknownSync(HistoryPageSchema);
const decodeViews = Schema.decodeUnknownSync(SavedViewsSchema);
const decodeImported = Schema.decodeUnknownSync(ViewsImportedSchema);

export async function openQuadratique(options: {
  readonly origin: string;
  readonly access: Access;
  readonly modelContext: WebMcp.ModelContext | undefined;
  readonly writable: () => boolean;
}): Promise<Quadratique> {
  const openings = new Map<string, () => void>();
  const layer = serverLayer(options.origin, options.access, (target, event) => {
    if (target === "web") {
      openings.get(event)?.();
    }
  });
  const page = await Handles.open(app, layer, { target: "web" });
  const agents =
    options.modelContext === undefined
      ? undefined
      : await Handles.open(app, layer, { target: "webmcp", webmcp: options.modelContext });

  const handle = (id: string) => {
    const found = page.handles.get(id);
    if (found === undefined) {
      throw new Error(`The web target has no capability ${id}`);
    }
    return found;
  };
  const call = async (id: string, input: object, signal: AbortSignal): Promise<unknown> => {
    if (isChanging(id) && !options.writable()) {
      throw new PastMoment(id);
    }
    const result = await handle(id).invokeExternal(input, { signal });
    if (!result.ok) {
      throw new Refused(id, result.error);
    }
    return result.value;
  };

  const matching = async (filter: IssueFilter, signal: AbortSignal) => {
    const all: IssueSummary[] = [];
    let cursor: string | undefined;
    do {
      // BOUNDARY: the answer of the server, already checked by the core against the catalog.
      const listed = decodePage(
        await call(
          "issue.list",
          { ...filter, limit: 200, ...(cursor === undefined ? {} : { cursor }) },
          signal,
        ),
      );
      all.push(...listed.items);
      cursor = listed.nextCursor;
    } while (cursor !== undefined);
    return all;
  };

  /** The occurrences of a web event, once the server has registered the subscription. */
  const subscribed = async (
    id: string,
    signal: AbortSignal,
    cursor: string | undefined,
  ): Promise<AsyncIterable<Occurrence<unknown>>> => {
    const event = page.events.get(id);
    if (event === undefined) {
      throw new Error(`The web target has no event ${id}`);
    }
    let confirm = (): void => {};
    const confirmed = new Promise<"opened">((resolve) => {
      confirm = () => {
        resolve("opened");
      };
    });
    openings.set(id, confirm);
    let iterator: AsyncIterator<Occurrence<unknown>>;
    let first: Promise<IteratorResult<Occurrence<unknown>>>;
    try {
      const occurrences = await event.subscribe(
        {},
        { signal, ...(cursor === undefined ? {} : { cursor }) },
      );
      iterator = occurrences[Symbol.asyncIterator]();
      first = iterator.next();
      await Promise.race([confirmed, first]);
    } finally {
      if (openings.get(id) === confirm) {
        openings.delete(id);
      }
    }
    return (async function* () {
      for (let next = await first; next.done !== true; next = await iterator.next()) {
        yield next.value;
      }
    })();
  };

  const changes = async (cursor: string | undefined, signal: AbortSignal) => {
    const occurrences = await subscribed("issue.changed", signal, cursor);
    return (async function* () {
      for await (const occurrence of occurrences) {
        yield {
          cursor: occurrence.cursor,
          truncated: occurrence.truncated === true,
          // BOUNDARY: the payload of the server, already checked by the core against the catalog.
          payload: decodeChange(occurrence.data),
        };
      }
    })();
  };

  const viewChanges = async (signal: AbortSignal) => {
    const occurrences = await subscribed("view.changed", signal, undefined);
    return (async function* () {
      for await (const occurrence of occurrences) {
        // BOUNDARY: the payload of the server, already checked by the core against the catalog.
        yield decodeViews(occurrence.data);
      }
    })();
  };

  /** Saved views belong to the human, not to a moment: they are written while the page shows the past too. */
  const viewCall = async (id: string, input: object, signal?: AbortSignal): Promise<unknown> => {
    const result = await handle(id).invokeExternal(input, signal === undefined ? {} : { signal });
    if (!result.ok) {
      throw new Refused(id, result.error);
    }
    // BOUNDARY: the answer of the server, already checked by the core against the catalog.
    return result.value;
  };

  const snapshot = (value: unknown): Snapshot => decodeViews(value);
  const views: Quadratique["views"] = {
    list: async (signal) => snapshot(await viewCall("view.list", {}, signal)),
    save: async (view) => snapshot(await viewCall("view.save", view)),
    rename: async (id, name) => snapshot(await viewCall("view.rename", { view: id, name })),
    remove: async (id) => snapshot(await viewCall("view.delete", { view: id })),
    import: async (source, imported) =>
      decodeImported(await viewCall("views.import", { source, views: imported })),
    changes: viewChanges,
  };

  return {
    projects: async (signal) => decodeProjects(await call("project.list", {}, signal)).items,
    head: async (signal, asOf) => {
      const [last] = decodeHistory(
        await call("history.query", { limit: 1, ...(asOf === undefined ? {} : { asOf }) }, signal),
      ).items;
      return last === undefined ? undefined : { commit: last.commit, at: last.at };
    },
    matching,
    issue: async (id, signal, asOf) =>
      decodeIssue(
        await call("issue.get", { issue: id, ...(asOf === undefined ? {} : { asOf }) }, signal),
      ).issue,
    history: async ({ issue, project, cursor }, signal) =>
      decodeHistory(
        await call(
          "history.query",
          {
            ...(issue === undefined ? {} : { issue }),
            ...(project === undefined ? {} : { project }),
            limit: 50,
            ...(cursor === undefined ? {} : { cursor }),
          },
          signal,
        ),
      ),
    update: async ({ issue, revision, patch }, signal) =>
      decodeSummary(await call("issue.update", { ...patch, issue, revision }, signal)).issue,
    create: async (issue, signal) => decodeSummary(await call("issue.create", issue, signal)).issue,
    link: async (request, signal) =>
      decodeRelations(await call("issue.link", request, signal)).relations,
    release: async (issue, signal) =>
      decodeSummary(await call("issue.release", { issue }, signal)).issue,
    finish: async (request, signal) =>
      decodeIssue(await call("issue.finish", request, signal)).issue,
    changes,
    views,
    dispose: async () => {
      await Promise.all([page.dispose(), agents?.dispose()]);
    },
  };
}
