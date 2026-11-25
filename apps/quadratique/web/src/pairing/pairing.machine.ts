import { createAsyncLogic, createCallbackLogic, setup, types } from "xstate";

/**
 * What the pairing needs from the page: the server's `POST /session`, the browser's storage, and the app itself.
 * docs/decisions/0004-a-socket-for-commands-and-a-token-for-the-browser.md, §4 "Pairing contract".
 */
export interface PairingPorts {
  /** The token for a one-time code; rejects when the server refuses the code or does not answer. */
  readonly redeem: (code: string) => Promise<string>;
  readonly save: (token: string) => void;
  /** Removes the saved token only when it is still `token`: another tab may have saved a newer one. */
  readonly drop: (token: string) => void;
  /**
   * Starts the app with `token`, and calls `unauthorized` when the server refuses it. The returned function ends the
   * app: its subscriptions, its WebMCP tools and its clients.
   */
  readonly open: (token: string, unauthorized: () => void) => () => void;
}

export interface PairingStart {
  /** The code of `#pair=` in the address the page was opened at. */
  readonly code: string | undefined;
  /** The token this browser saved for this address. */
  readonly saved: string | undefined;
}

/**
 * The page's pairing: `unpaired` (no token: the page shows the one sentence), `redeeming` (a code from `quadratique
 * ui` is being exchanged for the token), `paired` (the app runs with the token) and `refused` (the code was used or
 * expired, and no token was saved). A `401` from any target leaves `paired`; a token another tab saves enters it.
 */
export function pairingMachine(ports: PairingPorts, start: PairingStart) {
  const redeem = createAsyncLogic<string, { readonly code: string }>({
    run: async ({ input }) => await ports.redeem(input.code),
  });
  const app = createCallbackLogic<{ type: "never" }, { readonly token: string }>(
    ({ input, sendBack }) =>
      ports.open(input.token, () => {
        sendBack({ type: "unauthorized" });
      }),
  );
  const pairedWith = (token: string) => ({ target: "paired", context: { token } }) as const;

  return setup({
    schemas: {
      context: types<{ token: string | undefined; code: string | undefined }>(),
      events: {
        code: types<{ code: string }>(),
        unauthorized: types<void>(),
        stored: types<{ token: string }>(),
        cleared: types<void>(),
      },
    },
    actors: { redeem, app },
  }).createMachine({
    id: "pairing",
    context: { token: start.saved, code: start.code },
    initial: "starting",
    states: {
      starting: {
        always: ({ context }) => {
          if (context.code !== undefined) {
            return { target: "redeeming" };
          }
          return context.token === undefined ? { target: "unpaired" } : pairedWith(context.token);
        },
      },
      redeeming: {
        invoke: {
          src: "redeem",
          input: ({ context }) => ({ code: context.code ?? "" }),
          onDone: ({ event }, enq) => {
            enq(ports.save, event.output);
            return { target: "paired", context: { token: event.output, code: undefined } };
          },
          onError: ({ context }) =>
            context.token === undefined
              ? { target: "refused", context: { code: undefined } }
              : { target: "paired", context: { token: context.token, code: undefined } },
        },
        on: {
          stored: ({ event }) => ({ context: { token: event.token } }),
          cleared: () => ({ context: { token: undefined } }),
        },
      },
      paired: {
        invoke: {
          src: "app",
          input: ({ context }) => ({ token: context.token ?? "" }),
        },
        on: {
          unauthorized: ({ context }, enq) => {
            if (context.token !== undefined) {
              enq(ports.drop, context.token);
            }
            return { target: "unpaired", context: { token: undefined } };
          },
          stored: ({ context, event }) =>
            event.token === context.token
              ? undefined
              : { target: "paired", reenter: true, context: { token: event.token } },
          cleared: () => ({ target: "unpaired", context: { token: undefined } }),
          code: ({ event }) => ({ target: "redeeming", context: { code: event.code } }),
        },
      },
      unpaired: {
        on: {
          stored: ({ event }) => pairedWith(event.token),
          code: ({ event }) => ({ target: "redeeming", context: { code: event.code } }),
        },
      },
      refused: {
        on: {
          stored: ({ event }) => pairedWith(event.token),
          code: ({ event }) => ({ target: "redeeming", context: { code: event.code } }),
        },
      },
    },
  });
}

export type PairingPhase = "unpaired" | "redeeming" | "paired" | "refused";
