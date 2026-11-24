import type { IssueSummary, Project } from "../board/model.ts";

export const HANG_GUARD = { timeout: 30_000 };

export const demo: Project = {
  key: "DEM",
  name: "Demo",
  workflow: {
    statuses: [
      { key: "backlog", name: "Backlog", category: "backlog" },
      { key: "todo", name: "Todo", category: "unstarted" },
      { key: "doing", name: "Doing", category: "started" },
      { key: "checking", name: "Checking", category: "started" },
      { key: "landed", name: "Landed", category: "started" },
      { key: "done", name: "Done", category: "completed" },
    ],
    moves: { claim: "doing", review: "checking", merge: "landed", release: "todo" },
  },
};

export function summary(id: string, fields: Partial<IssueSummary> = {}): IssueSummary {
  return {
    id,
    project: "DEM",
    title: `Issue ${id}`,
    status: "todo",
    category: "unstarted",
    priority: 0,
    labels: [],
    ready: true,
    blockedBy: [],
    order: 1,
    revision: 1,
    ...fields,
  };
}

export class Calls<T> {
  private readonly received: T[] = [];
  private readonly waiting = new Map<number, (call: T) => void>();

  add(call: T): void {
    this.received.push(call);
    this.waiting.get(this.received.length)?.(call);
  }

  async nth(count: number): Promise<T> {
    const known = this.received[count - 1];
    if (known !== undefined) {
      return known;
    }
    return await new Promise<T>((resolve) => {
      this.waiting.set(count, resolve);
    });
  }
}
