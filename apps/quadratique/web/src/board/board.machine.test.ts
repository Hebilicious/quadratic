import { describe, expect, onTestFinished, test } from "vitest";
import { createActor, waitFor } from "xstate";

import type { Change, Head, Quadratique } from "../core/quadratique.ts";
import { Calls, HANG_GUARD, summary, demo } from "../test/fixtures.ts";
import { boardMachine } from "./board.machine.ts";
import { projectScope } from "./view.ts";
import type { IssueSummary } from "./model.ts";

const at = (second: number) => `2026-11-18T10:00:${String(second).padStart(2, "0")}.000Z`;

class Stream {
  private push: ((change: Change) => void) | undefined;
  private announce: (() => void) | undefined;
  readonly opened = new Promise<void>((resolve) => {
    this.announce = resolve;
  });

  readonly changes: Quadratique["changes"] = async (_cursor, signal) => {
    const queue: Change[] = [];
    let wake: (() => void) | undefined;
    this.push = (change) => {
      queue.push(change);
      wake?.();
    };
    this.announce?.();
    signal.addEventListener("abort", () => wake?.(), { once: true });
    return (async function* () {
      while (!signal.aborted) {
        const next = queue.shift();
        if (next === undefined) {
          await new Promise<void>((resolve) => {
            wake = resolve;
          });
        } else {
          yield next;
        }
      }
    })();
  };

  send(change: Change): void {
    this.push?.(change);
  }
}

const change = (second: number, issues: readonly IssueSummary[]): Change => ({
  cursor: `c${String(second)}`,
  truncated: false,
  payload: {
    commit: `c${String(second)}`,
    at: at(second),
    operation: "issue.link",
    author: { kind: "agent", name: "claude-code@dem-1" },
    issues: issues.map((issue) => ({ issue })),
    projects: [],
  },
});

interface Read {
  readonly answer: (items: readonly IssueSummary[]) => void;
}

function board() {
  const stream = new Stream();
  const reads = new Calls<Read>();
  const server: { head: Head } = { head: { commit: "c0", at: at(0) } };
  const actor = createActor(
    boardMachine({
      projects: async () => [demo],
      head: async () => server.head,
      matching: async (filter) => {
        if (filter.project === undefined) {
          return [];
        }
        return await new Promise<readonly IssueSummary[]>((resolve) => {
          reads.add({ answer: resolve });
        });
      },
      issue: async () => {
        throw new Error("The board's tests open no issue");
      },
      history: async () => ({ items: [] }),
      changes: stream.changes,
    }),
  );
  actor.start();
  onTestFinished(() => {
    actor.stop();
  });
  return { actor, stream, server, read: async (count: number) => await reads.nth(count) };
}

describe("the board recovers from a gap in issue.changed", () => {
  const gaps: readonly (readonly [string, (change: Change) => Change])[] = [
    ["truncated", (each) => ({ ...each, truncated: true })],
    ["cut", (each) => ({ ...each, payload: { ...each.payload, cut: true } })],
  ];

  test.each(gaps)(
    "after a %s occurrence, it shows the issues read again, not a change it held from before the gap",
    async (_name, gap) => {
      const { actor, stream, server, read } = board();
      await stream.opened;
      actor.send({ type: "select", scope: projectScope("DEM"), at: undefined });
      await read(1);

      stream.send(change(1, [summary("DEM-1", { revision: 2, ready: false })]));
      server.head = { commit: "c2", at: at(2) };
      stream.send(gap(change(2, [])));
      (await read(2)).answer([summary("DEM-1", { revision: 2, ready: true })]);

      await waitFor(actor, (snapshot) => snapshot.matches({ issues: "ready" }), HANG_GUARD);
      expect(actor.getSnapshot().context.issues["DEM-1"]).toMatchObject({
        revision: 2,
        ready: true,
      });
    },
  );
});

describe("the board orders what it learns", () => {
  test("the late answer of a change does not undo a newer change of the same revision", async () => {
    const { actor, stream, read } = board();
    await stream.opened;
    actor.send({ type: "select", scope: projectScope("DEM"), at: undefined });
    (await read(1)).answer([summary("DEM-1")]);
    await waitFor(actor, (snapshot) => snapshot.matches({ issues: "ready" }), HANG_GUARD);

    stream.send(change(1, [summary("DEM-1", { revision: 2, ready: true, status: "doing" })]));
    await waitFor(
      actor,
      (snapshot) => snapshot.context.issues["DEM-1"]?.revision === 2,
      HANG_GUARD,
    );
    actor.send({
      type: "settled",
      issues: [summary("DEM-1", { revision: 2, ready: false, status: "doing" })],
    });

    expect(actor.getSnapshot().context.issues["DEM-1"]?.ready).toBe(true);
  });
});

describe("the board at a moment of the past", () => {
  test("it shows the issues as they were then, ignores the live changes, and back to now shows them as they are", async () => {
    const stream = new Stream();
    const asked: (string | undefined)[] = [];
    const actor = createActor(
      boardMachine({
        projects: async () => [demo],
        head: async (_signal, asOf) => {
          asked.push(asOf);
          return asOf === undefined ? { commit: "c5", at: at(5) } : { commit: "c1", at: at(1) };
        },
        matching: async (filter) =>
          filter.project === undefined
            ? []
            : [summary("DEM-1", filter.asOf === "c1" ? {} : { status: "done", revision: 5 })],
        issue: async () => {
          throw new Error("The board's tests open no issue");
        },
        history: async () => ({ items: [] }),
        changes: stream.changes,
      }),
    );
    actor.start();
    onTestFinished(() => {
      actor.stop();
    });
    await stream.opened;

    actor.send({ type: "select", scope: projectScope("DEM"), at: at(1).replace(".000", "") });
    await waitFor(actor, (snapshot) => snapshot.matches({ issues: "past" }), HANG_GUARD);
    expect(actor.getSnapshot().context.moment).toEqual({ commit: "c1", at: at(1) });
    expect(actor.getSnapshot().context.issues["DEM-1"]?.status).toBe("todo");

    stream.send(change(4, [summary("DEM-1", { status: "doing", revision: 4 })]));
    const connection = actor.getSnapshot().children.connection;
    if (connection === undefined) {
      throw new Error("The board runs no live connection");
    }
    await waitFor(connection, (snapshot) => snapshot.context.cursor === "c4", HANG_GUARD);
    expect(actor.getSnapshot().context.issues["DEM-1"]?.status).toBe("todo");

    actor.send({ type: "select", scope: projectScope("DEM"), at: undefined });
    await waitFor(actor, (snapshot) => snapshot.matches({ issues: "ready" }), HANG_GUARD);
    expect(actor.getSnapshot().context.issues["DEM-1"]?.status).toBe("done");
    expect(asked).toContain("2026-11-18T10:00:01Z");
  });
});

/** A board that reads without end spins on promises alone, which starves the timer of a hang guard: stop it instead. */
const SPIN_GUARD = 10_000;

function milestoneBoard() {
  const stream = new Stream();
  const server = { milestone: "m1", head: { commit: "c0", at: at(0) } };
  const held: { release: (() => void) | undefined; next: boolean; fails: boolean } = {
    release: undefined,
    next: false,
    fails: false,
  };
  const reads = new Calls<string>();
  const actor = createActor(
    boardMachine({
      projects: async () => [demo],
      head: async () => server.head,
      matching: async (filter) => (filter.project === "DEM" ? [summary("DEM-1")] : []),
      issue: async (id) => {
        const milestone = server.milestone;
        reads.add(milestone);
        if (held.fails) {
          throw new Error("The server could not be reached");
        }
        if (held.next) {
          held.next = false;
          await new Promise<void>((resolve) => {
            held.release = resolve;
          });
        }
        return {
          ...summary(id),
          milestone,
          notes: [],
          relations: [],
          children: [],
          createdAt: at(0),
          createdBy: { kind: "human", name: "tester" },
          updatedAt: at(0),
          links: [],
        };
      },
      history: async () => ({ items: [] }),
      changes: stream.changes,
    }),
  );
  actor.start();
  onTestFinished(() => {
    actor.stop();
  });
  let snapshots = 0;
  actor.subscribe(() => {
    snapshots += 1;
    if (snapshots > SPIN_GUARD) {
      actor.stop();
    }
  });
  const settledOn = async (milestone: string, cursor: string | undefined) =>
    await waitFor(
      actor,
      (snapshot) =>
        snapshot.matches({ milestones: { on: "read" } }) &&
        snapshot.context.milestones["DEM-1"] === milestone &&
        snapshot.children.connection?.getSnapshot().context.cursor === cursor,
      HANG_GUARD,
    );
  return { actor, stream, server, held, reads, settledOn };
}

describe("the milestones of a board grouped by milestone", () => {
  test("a change to an issue of another project leaves the board read, with nothing left to read", async () => {
    const { actor, stream, settledOn } = milestoneBoard();
    await stream.opened;
    actor.send({ type: "select", scope: projectScope("DEM"), at: undefined });
    actor.send({ type: "milestones", wanted: true });
    await settledOn("m1", undefined);
    stream.send(change(1, [summary("QUA-1", { project: "QUA" })]));
    await settledOn("m1", "c1");
  });

  test("a milestone changed while the view grouped by something else shows when it groups by milestone again", async () => {
    const { actor, stream, server, settledOn } = milestoneBoard();
    await stream.opened;
    actor.send({ type: "select", scope: projectScope("DEM"), at: undefined });
    actor.send({ type: "milestones", wanted: true });
    await settledOn("m1", undefined);
    actor.send({ type: "milestones", wanted: false });
    server.milestone = "m2";
    stream.send(change(1, [summary("DEM-1", { revision: 2 })]));
    await waitFor(
      actor,
      (snapshot) => snapshot.children.connection?.getSnapshot().context.cursor === "c1",
      HANG_GUARD,
    );
    actor.send({ type: "milestones", wanted: true });
    await settledOn("m2", "c1");
  });

  test("a change that arrives while the milestone is read is not lost to the older answer", async () => {
    const { actor, stream, server, held, reads, settledOn } = milestoneBoard();
    await stream.opened;
    actor.send({ type: "select", scope: projectScope("DEM"), at: undefined });
    actor.send({ type: "milestones", wanted: true });
    await settledOn("m1", undefined);
    held.next = true;
    stream.send(change(1, [summary("DEM-1", { revision: 2 })]));
    await reads.nth(2);
    server.milestone = "m2";
    stream.send(change(2, [summary("DEM-1", { revision: 3 })]));
    await waitFor(
      actor,
      (snapshot) => snapshot.children.connection?.getSnapshot().context.cursor === "c2",
      HANG_GUARD,
    );
    held.release?.();
    await settledOn("m2", "c2");
  });

  test("a milestone read that fails is read again on Retry, once the server answers", async () => {
    const { actor, stream, server, held, settledOn } = milestoneBoard();
    await stream.opened;
    actor.send({ type: "select", scope: projectScope("DEM"), at: undefined });
    actor.send({ type: "milestones", wanted: true });
    await settledOn("m1", undefined);
    server.milestone = "m2";
    held.fails = true;
    stream.send(change(1, [summary("DEM-1", { revision: 2 })]));
    await waitFor(
      actor,
      (snapshot) => snapshot.matches({ milestones: { on: "failed" } }),
      HANG_GUARD,
    );
    held.fails = false;
    actor.send({ type: "retry" });
    await settledOn("m2", "c1");
  });

  test("after a gap in issue.changed, the milestones are read again with the issues", async () => {
    const { actor, stream, server, settledOn } = milestoneBoard();
    await stream.opened;
    actor.send({ type: "select", scope: projectScope("DEM"), at: undefined });
    actor.send({ type: "milestones", wanted: true });
    await settledOn("m1", undefined);
    server.milestone = "m2";
    server.head = { commit: "c1", at: at(1) };
    stream.send({ ...change(1, []), truncated: true });
    await settledOn("m2", "c1");
  });
});

function historyBoard() {
  const stream = new Stream();
  const server = { olderFails: true, newest: ["c2"] };
  const entry = (commit: string) => ({
    commit,
    at: at(1),
    author: { kind: "human" as const, name: "tester" },
    operation: "issue.update",
    project: "DEM",
    issue: "DEM-1",
    fields: [],
  });
  const actor = createActor(
    boardMachine({
      projects: async () => [demo],
      head: async () => ({ commit: "c0", at: at(0) }),
      matching: async () => [],
      issue: async () => {
        throw new Error("The board's tests open no issue");
      },
      history: async ({ cursor }) => {
        if (cursor === undefined) {
          return { items: server.newest.map(entry), nextCursor: "c2" };
        }
        if (server.olderFails) {
          throw new Error("The server could not be reached");
        }
        return { items: [entry("c1")] };
      },
      changes: stream.changes,
    }),
  );
  actor.start();
  onTestFinished(() => {
    actor.stop();
  });
  return { actor, stream, server };
}

describe("the history of a project", () => {
  test("when the older changes fail to load, the page knows it, and asking again loads them", async () => {
    const { actor, server } = historyBoard();
    actor.send({ type: "history", scope: projectScope("DEM") });
    await waitFor(actor, (snapshot) => snapshot.matches({ history: "shown" }), HANG_GUARD);
    actor.send({ type: "moreChanges" });
    await waitFor(
      actor,
      (snapshot) => snapshot.matches({ history: { shown: { older: "failed" } } }),
      HANG_GUARD,
    );
    server.olderFails = false;
    actor.send({ type: "moreChanges" });
    await waitFor(actor, (snapshot) => snapshot.context.changes.length === 2, HANG_GUARD);
    expect(actor.getSnapshot().context.changes.map((each) => each.commit)).toEqual(["c2", "c1"]);
  });

  test("a live change read while the older changes failed keeps the failure and its Retry", async () => {
    const { actor, stream, server } = historyBoard();
    await stream.opened;
    actor.send({ type: "history", scope: projectScope("DEM") });
    await waitFor(actor, (snapshot) => snapshot.matches({ history: "shown" }), HANG_GUARD);
    actor.send({ type: "moreChanges" });
    await waitFor(
      actor,
      (snapshot) => snapshot.matches({ history: { shown: { older: "failed" } } }),
      HANG_GUARD,
    );
    server.newest = ["c3", "c2"];
    stream.send(change(3, [summary("DEM-1", { revision: 3 })]));
    await waitFor(actor, (snapshot) => snapshot.context.changes[0]?.commit === "c3", HANG_GUARD);
    expect(actor.getSnapshot().matches({ history: { shown: { older: "failed" } } })).toBe(true);
  });
});
