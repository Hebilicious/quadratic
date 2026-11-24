import { describe, expect, onTestFinished, test } from "vitest";
import { createActor, waitFor } from "xstate";

import { Refused } from "../core/quadratic.ts";
import { HANG_GUARD } from "../test/fixtures.ts";
import { Drafts } from "./drafts.ts";
import { editMachine } from "./edit.machine.ts";

function notesPage() {
  const server = { revision: 3, notes: [] as string[] };
  const page = { revision: 3, notes: [] as string[] };
  const drafts = new Drafts();
  const key = "ABC-1 Notes";
  let answer: (() => void) | undefined;
  let holdAnswers = false;
  const open = () => {
    const actor = createActor(
      editMachine({
        run: async (base) => {
          const text = drafts.get(key)?.text ?? "";
          if (base !== server.revision) {
            throw new Refused("issue.update", { _tag: "Stale", revision: server.revision });
          }
          server.notes.push(text);
          server.revision += 1;
          if (holdAnswers) {
            await new Promise<void>((resolve) => {
              answer = resolve;
            });
          }
        },
        revision: () => page.revision,
        refresh: () => undefined,
        saved: () => {
          drafts.drop(key);
        },
        landed: () => page.notes.at(-1) === drafts.get(key)?.text,
      }),
    );
    actor.start();
    onTestFinished(() => {
      actor.stop();
    });
    const kept = drafts.get(key);
    if (kept !== undefined) {
      actor.send({ type: "resume", base: kept.base });
    }
    return actor;
  };
  return {
    server,
    page,
    drafts,
    open,
    type: (text: string) => {
      drafts.keep(key, { text, base: page.revision });
    },
    holdAnswers: () => {
      holdAnswers = true;
    },
    answerLate: () => {
      holdAnswers = false;
      answer?.();
    },
    read: () => {
      page.revision = server.revision;
      page.notes = [...server.notes];
    },
    key,
  };
}

async function leaveWhileSaving(notes: ReturnType<typeof notesPage>): Promise<void> {
  notes.holdAnswers();
  const first = notes.open();
  first.send({ type: "start" });
  notes.type("Ship it");
  first.send({ type: "save" });
  await waitFor(first, () => notes.server.notes.length === 1, HANG_GUARD);
  first.stop();
  notes.answerLate();
}

describe("an edit of an issue", () => {
  test("an edit refused as stale is not saved again until the page has read the issue as it is now", async () => {
    const server = { revision: 4 };
    const page = { revision: 3 };
    const sent: number[] = [];
    const actor = createActor(
      editMachine({
        run: async (base) => {
          sent.push(base);
          if (base !== server.revision) {
            throw new Refused("issue.update", { _tag: "Stale", revision: server.revision });
          }
          server.revision += 1;
        },
        revision: () => page.revision,
        refresh: () => undefined,
      }),
    );
    actor.start();
    onTestFinished(() => {
      actor.stop();
    });
    actor.send({ type: "submit" });
    await waitFor(actor, (snapshot) => snapshot.matches("stale"), HANG_GUARD);

    actor.send({ type: "save" });
    expect(sent).toEqual([3]);

    page.revision = 4;
    actor.send({ type: "revised", revision: 4 });
    actor.send({ type: "save" });
    await waitFor(actor, (snapshot) => snapshot.matches("idle"), HANG_GUARD);
    expect(server.revision).toBe(5);
  });

  test("a note saved while the page was left is not added again when the page comes back on its old revision", async () => {
    const notes = notesPage();
    await leaveWhileSaving(notes);

    const again = notes.open();
    again.send({ type: "save" });
    await waitFor(again, (snapshot) => snapshot.matches("stale"), HANG_GUARD);
    notes.read();
    again.send({ type: "revised", revision: notes.page.revision });
    again.send({ type: "save" });
    await waitFor(again, (snapshot) => !snapshot.matches("saving"), HANG_GUARD);

    expect(notes.server.notes).toEqual(["Ship it"]);
    expect(notes.drafts.get(notes.key)).toBeUndefined();
  });

  test("a note saved while the page was left is not restored as a draft once the page has read it", async () => {
    const notes = notesPage();
    await leaveWhileSaving(notes);
    notes.read();

    const again = notes.open();

    expect(again.getSnapshot().matches("idle")).toBe(true);
    expect(notes.drafts.get(notes.key)).toBeUndefined();
  });
});
