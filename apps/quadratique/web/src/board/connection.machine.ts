import { createCallbackLogic, setup, types } from "xstate";

import type { Change, Quadratique } from "../core/quadratique.ts";
import type { IssueChanged } from "./model.ts";

export type ConnectionNews = { type: "changed"; payload: IssueChanged } | { type: "resync" };

const MAX_RETRY_MS = 10_000;

export function connectionMachine(quadratique: Pick<Quadratique, "changes">) {
  const follow = createCallbackLogic<{ type: "never" }, { readonly cursor: string | undefined }>(
    ({ input, sendBack }) => {
      const abort = new AbortController();
      const forwardEveryChange = async () => {
        const changes = await quadratique.changes(input.cursor, abort.signal);
        sendBack({ type: "opened" });
        for await (const change of changes) {
          sendBack({ type: "change", change });
        }
      };
      const streamEndedWhetherCompletedRefusedOrBroken = () => {
        if (!abort.signal.aborted) {
          sendBack({ type: "lost" });
        }
      };
      void forwardEveryChange().then(
        streamEndedWhetherCompletedRefusedOrBroken,
        streamEndedWhetherCompletedRefusedOrBroken,
      );
      return () => {
        abort.abort();
      };
    },
  );

  return setup({
    schemas: {
      context: types<{ cursor: string | undefined; attempts: number }>(),
      events: {
        opened: types<void>(),
        change: types<{ change: Change }>(),
        lost: types<void>(),
      },
    },
    actors: { follow },
    delays: {
      retry: ({ context }) => Math.min(500 * 2 ** context.attempts, MAX_RETRY_MS),
    },
  }).createMachine({
    id: "connection",
    context: { cursor: undefined, attempts: 0 },
    initial: "following",
    states: {
      following: {
        invoke: { src: "follow", input: ({ context }) => ({ cursor: context.cursor }) },
        initial: "connecting",
        states: {
          connecting: {
            on: {
              opened: ({ context, parent }, enq) => {
                if (context.cursor === undefined) {
                  enq.sendTo(parent, { type: "resync" } satisfies ConnectionNews);
                }
                return { target: "live", context: { attempts: 0 } };
              },
            },
          },
          live: {},
        },
        on: {
          change: ({ context, event, parent }, enq) => {
            const { change } = event;
            const news: ConnectionNews =
              change.truncated || change.payload.cut === true
                ? { type: "resync" }
                : { type: "changed", payload: change.payload };
            enq.sendTo(parent, news);
            return { context: { cursor: change.cursor ?? context.cursor } };
          },
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

export type ConnectionPhase = "connecting" | "live" | "reconnecting";

export function connectionPhase(snapshot: {
  matches: (state: "reconnecting" | { following: "live" }) => boolean;
}): ConnectionPhase {
  if (snapshot.matches("reconnecting")) {
    return "reconnecting";
  }
  return snapshot.matches({ following: "live" }) ? "live" : "connecting";
}
