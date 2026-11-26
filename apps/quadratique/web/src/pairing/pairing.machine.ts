import { createAsyncLogic, createCallbackLogic, setup, types, type StateValue } from "xstate";

/** The running app, as the pairing drives it. */
export interface AppSession {
  /** Ends the app: its subscriptions, its WebMCP tools and its clients. */
  readonly end: () => void;
  /** The token for the app's later requests: its clients read the current one at each request. */
  readonly use: (token: string) => void;
}

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
  /** Starts the app with `token`, and calls `unauthorized` when the server refuses the token the app uses. */
  readonly open: (token: string, unauthorized: () => void) => AppSession;
}

export interface PairingStart {
  /** The code of `#pair=` in the address the page was opened at. */
  readonly code: string | undefined;
  /** The token this browser saved for this address. */
  readonly saved: string | undefined;
}

/** Whether the pairing is exchanging a code after the token of the running app was refused or cleared. */
const exchangingAfterLoss = (value: StateValue): boolean =>
  typeof value === "object" &&
  typeof value["paired"] === "object" &&
  value["paired"]["exchanging"] === "refused";

/**
 * The page's pairing: `unpaired` (no token: the page shows the one sentence), `redeeming` (a code from `quadratique
 * ui` is being exchanged for the token), `paired` (the app runs with the token) and `refused` (the code was used or
 * expired, and no token was saved). A later code replaces one that is being redeemed.
 *
 * In `paired`, a code is exchanged while the app keeps running (`exchanging`), so nothing the person typed is lost. A
 * new token from it goes to the running app for its later requests: the server holds one token, so the old one is
 * stale. A `401` or a cleared token during the exchange (`exchanging.refused`) drops the old token and waits for the
 * exchange: its token keeps the app running, its refusal leaves the tab unpaired. A `401` while no code is exchanged
 * ends the app.
 */
export function pairingMachine(ports: PairingPorts, start: PairingStart) {
  const redeem = createAsyncLogic<string, { readonly code: string }>({
    run: async ({ input }) => await ports.redeem(input.code),
  });
  const app = createCallbackLogic<{ type: "use"; token: string }, { readonly token: string }>(
    ({ input, sendBack, receive }) => {
      const session = ports.open(input.token, () => {
        sendBack({ type: "unauthorized" });
      });
      receive((event) => {
        session.use(event.token);
      });
      return session.end;
    },
  );
  const pairedWith = (token: string) => ({ target: "paired", context: { token } }) as const;
  const exchangeCode = ({ event }: { event: { code: string } }) =>
    ({ target: "redeeming", context: { code: event.code } }) as const;

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
          code: ({ event }) => ({
            target: "redeeming",
            reenter: true,
            context: { code: event.code },
          }),
          stored: ({ event }) => ({ context: { token: event.token } }),
          cleared: () => ({ context: { token: undefined } }),
        },
      },
      paired: {
        invoke: {
          id: "app",
          src: "app",
          input: ({ context }) => ({ token: context.token ?? "" }),
        },
        initial: "running",
        states: {
          running: {
            on: {
              code: ({ event }) => ({ target: "exchanging", context: { code: event.code } }),
              unauthorized: ({ context }, enq) => {
                if (context.token !== undefined) {
                  enq(ports.drop, context.token);
                }
                return { target: "#pairing.unpaired", context: { token: undefined } };
              },
              cleared: () => ({ target: "#pairing.unpaired", context: { token: undefined } }),
            },
          },
          exchanging: {
            invoke: {
              src: "redeem",
              input: ({ context }) => ({ code: context.code ?? "" }),
              onDone: ({ context, event }, enq) => {
                if (event.output !== context.token) {
                  enq(ports.save, event.output);
                  enq.sendTo("app", { type: "use", token: event.output });
                }
                return {
                  target: "#pairing.paired.running",
                  context: { token: event.output, code: undefined },
                };
              },
              onError: ({ value }) =>
                exchangingAfterLoss(value)
                  ? { target: "#pairing.unpaired", context: { token: undefined, code: undefined } }
                  : { target: "#pairing.paired.running", context: { code: undefined } },
            },
            initial: "trusted",
            states: {
              trusted: {
                on: {
                  unauthorized: ({ context }, enq) => {
                    if (context.token !== undefined) {
                      enq(ports.drop, context.token);
                    }
                    return { target: "refused", context: { token: undefined } };
                  },
                  cleared: () => ({ target: "refused", context: { token: undefined } }),
                },
              },
              // The app's token is gone: a later code is redeemed as in an unpaired tab.
              refused: {
                on: {
                  code: ({ event }) => ({
                    target: "#pairing.redeeming",
                    context: { code: event.code },
                  }),
                },
              },
            },
            on: {
              code: ({ event }) => ({
                target: "exchanging",
                reenter: true,
                context: { code: event.code },
              }),
            },
          },
        },
        on: {
          stored: ({ context, event }) =>
            event.token === context.token
              ? undefined
              : { target: "paired", reenter: true, context: { token: event.token } },
        },
      },
      unpaired: {
        on: {
          stored: ({ event }) => pairedWith(event.token),
          code: exchangeCode,
        },
      },
      refused: {
        on: {
          stored: ({ event }) => pairedWith(event.token),
          code: exchangeCode,
        },
      },
    },
  });
}

export type PairingPhase = "unpaired" | "redeeming" | "paired" | "refused";
