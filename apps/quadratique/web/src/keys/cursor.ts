export type Layout = readonly (readonly string[])[];

interface Position {
  readonly column: number;
  readonly row: number;
}

function positionOf(layout: Layout, id: string | undefined): Position | undefined {
  if (id === undefined) {
    return undefined;
  }
  for (const [column, ids] of layout.entries()) {
    const row = ids.indexOf(id);
    if (row !== -1) {
      return { column, row };
    }
  }
  return undefined;
}

const at = (layout: Layout, column: number, row: number): string | undefined => {
  const ids = layout[column];
  if (ids === undefined || ids.length === 0) {
    return undefined;
  }
  return ids[Math.min(Math.max(row, 0), ids.length - 1)];
};

const firstIssue = (layout: Layout): string | undefined =>
  layout.find((ids) => ids.length > 0)?.[0];

export function vertical(
  layout: Layout,
  cursor: string | undefined,
  by: -1 | 1,
): string | undefined {
  const position = positionOf(layout, cursor);
  if (position === undefined) {
    return firstIssue(layout);
  }
  return at(layout, position.column, position.row + by);
}

export function horizontal(
  layout: Layout,
  cursor: string | undefined,
  by: -1 | 1,
): string | undefined {
  const position = positionOf(layout, cursor);
  if (position === undefined) {
    return firstIssue(layout);
  }
  for (let column = position.column + by; column >= 0 && column < layout.length; column += by) {
    const found = at(layout, column, position.row);
    if (found !== undefined) {
      return found;
    }
  }
  return cursor;
}

export function followed(
  before: Layout,
  after: Layout,
  cursor: string | undefined,
): string | undefined {
  if (cursor === undefined || positionOf(after, cursor) !== undefined) {
    return cursor;
  }
  const was = positionOf(before, cursor);
  if (was === undefined) {
    return cursor;
  }
  return at(after, was.column, was.row) ?? firstIssue(after);
}

export function contains(layout: Layout, id: string): boolean {
  return positionOf(layout, id) !== undefined;
}
