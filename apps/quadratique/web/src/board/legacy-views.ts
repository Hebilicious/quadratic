import catalog from "quadratique-catalog/quadratique.acidcat.json" with { type: "json" };
import { Option, Schema } from "effect";

import { firstCharacters } from "../route.ts";

const types = catalog.$defs;
const importInput = catalog.capabilities["views.import"].input.properties;

/** What the server takes, from the catalog, so the page never sends a row that the whole call would be refused for. */
export const IMPORT_LIMITS = {
  name: types.SavedViewName.maxLength,
  filter: types.SavedViewFilter.maxLength,
  project: new RegExp(types.ProjectKey.pattern, "u"),
  old: importInput.views.items.properties.old.maxLength,
  source: new RegExp(importInput.source.pattern, "u"),
  batchViews: importInput.views.maxItems,
  /** Far under the server's request limit of 1 MiB, whatever the rows hold. */
  batchBytes: 256 * 1024,
} as const;

/** An old view as `views.import` takes it. */
export interface OldView {
  readonly old: string;
  readonly name: string;
  readonly project?: string;
  readonly filter: string;
}

/** An old view that stays in this browser, and why. */
export interface Kept {
  readonly name: string;
  readonly reason: string;
}

/** An old view whose name was cut to the longest name the server keeps. */
export interface Shortened {
  readonly from: string;
  readonly to: string;
}

const LegacySchema = Schema.Struct({
  version: Schema.Literal(1),
  source: Schema.optionalKey(Schema.String),
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

export type LegacyPlan =
  /** Nothing under the old key. */
  | { readonly kind: "none" }
  /** A value whose shape this page does not know: it stays as it is, and nothing is sent. */
  | { readonly kind: "unknown" }
  | {
      readonly kind: "import";
      /** The value as read: only this exact value is removed once every view is in the server. */
      readonly raw: string;
      /** The id of these old views that an earlier page made, or none yet. */
      readonly source: string | undefined;
      readonly rows: readonly OldView[];
      readonly kept: readonly Kept[];
      readonly shortened: readonly Shortened[];
    };

function parsed(raw: string): unknown {
  try {
    // BOUNDARY: what this browser stored; the schema below decodes it.
    return JSON.parse(raw);
  } catch {
    return undefined;
  }
}

/** What to import from the value under the old key, and what stays in the browser. */
export function planLegacy(raw: string | null): LegacyPlan {
  if (raw === null) {
    return { kind: "none" };
  }
  const decoded = decodeLegacy(parsed(raw));
  if (Option.isNone(decoded)) {
    return { kind: "unknown" };
  }
  const rows: OldView[] = [];
  const kept: Kept[] = [];
  const shortened: Shortened[] = [];
  const seen = new Set<string>();
  for (const view of decoded.value.views) {
    const name = view.name.trim();
    const keep = (reason: string) => {
      kept.push({ name: name.length === 0 ? view.id : name, reason });
    };
    if (view.id.length === 0 || view.id.length > IMPORT_LIMITS.old || seen.has(view.id)) {
      keep("its id in this browser is empty, too long or used twice");
      continue;
    }
    seen.add(view.id);
    if (name.length === 0) {
      keep("it has no name");
    } else if (view.project !== undefined && !IMPORT_LIMITS.project.test(view.project)) {
      keep(`${view.project} is not a project key`);
    } else if (view.filter.length > IMPORT_LIMITS.filter) {
      keep(`its filters are longer than ${String(IMPORT_LIMITS.filter)} characters`);
    } else {
      const cut = firstCharacters(name, IMPORT_LIMITS.name).trimEnd();
      if (cut !== name) {
        shortened.push({ from: name, to: cut });
      }
      rows.push({
        old: view.id,
        name: cut,
        ...(view.project === undefined ? {} : { project: view.project }),
        filter: view.filter,
      });
    }
  }
  const source = decoded.value.source;
  return {
    kind: "import",
    raw,
    source: source !== undefined && IMPORT_LIMITS.source.test(source) ? source : undefined,
    rows,
    kept,
    shortened,
  };
}

/** The value under the old key with the id of its old views, which the page writes once before the first import. */
export function withSource(raw: string, source: string): string {
  const value = parsed(raw);
  return JSON.stringify({ ...(typeof value === "object" && value !== null ? value : {}), source });
}

const bytes = (row: OldView): number => new TextEncoder().encode(JSON.stringify(row)).length;

/** The rows in calls of at most the catalog's number of views and far under the request limit, in their order. */
export function batches(rows: readonly OldView[]): readonly (readonly OldView[])[] {
  const all: OldView[][] = [];
  let current: OldView[] = [];
  let size = 0;
  for (const row of rows) {
    const rowSize = bytes(row);
    if (
      current.length > 0 &&
      (current.length >= IMPORT_LIMITS.batchViews || size + rowSize > IMPORT_LIMITS.batchBytes)
    ) {
      all.push(current);
      current = [];
      size = 0;
    }
    current.push(row);
    size += rowSize;
  }
  if (current.length > 0) {
    all.push(current);
  }
  return all;
}
