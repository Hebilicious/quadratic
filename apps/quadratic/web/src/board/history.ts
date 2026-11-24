import type { HistoryChange, HistoryPage } from "./model.ts";

export const changeKey = (change: HistoryChange): string =>
  `${change.commit}:${change.issue ?? ""}`;

export function moreChangesThanOnePageHolds(
  known: { readonly changes: readonly HistoryChange[] },
  page: HistoryPage,
): boolean {
  const keys = new Set(known.changes.map(changeKey));
  return page.nextCursor !== undefined && !page.items.some((change) => keys.has(changeKey(change)));
}

export function prepended(
  known: { readonly changes: readonly HistoryChange[]; readonly changesCursor: string | undefined },
  page: HistoryPage,
): { readonly changes: readonly HistoryChange[]; readonly changesCursor: string | undefined } {
  const keys = new Set(known.changes.map(changeKey));
  if (moreChangesThanOnePageHolds(known, page)) {
    return { changes: page.items, changesCursor: page.nextCursor };
  }
  const overlap = page.items.findIndex((change) => keys.has(changeKey(change)));
  const newer = overlap === -1 ? page.items : page.items.slice(0, overlap);
  return { changes: [...newer, ...known.changes], changesCursor: known.changesCursor };
}
