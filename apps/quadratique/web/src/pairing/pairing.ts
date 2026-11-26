/** The key under which the browser keeps the token for this address. Nothing else of the person's lives in it. */
export const TOKEN_KEY = "quadratique.token";

/** The one sentence of a page that is not paired: the server says the same in its `401`. */
export const NOT_PAIRED_BEFORE = "To open Quadratique in this browser, run ";
export const PAIR_COMMAND = "quadratique ui --open";
export const NOT_PAIRED_AFTER = " in a terminal.";

const PAIR_FRAGMENT = /^#pair=([A-Za-z0-9_-]{1,128})(?:&(.*))?$/u;

/**
 * The one-time code of `#pair=<code>`, read and cleared from the address before anything else reads it, so that it
 * stays neither in the address bar nor in the session history. The page's own fragment may follow the code
 * (`#pair=<code>&finish`): it stays in the address as `#finish`.
 */
export function takeCode(view: {
  readonly location: Pick<Location, "hash" | "pathname" | "search">;
  readonly history: Pick<History, "replaceState" | "state">;
}): string | undefined {
  const { hash, pathname, search } = view.location;
  if (!hash.startsWith("#pair=")) {
    return undefined;
  }
  const found = PAIR_FRAGMENT.exec(hash);
  const own = found?.[2];
  view.history.replaceState(
    view.history.state,
    "",
    pathname + search + (own === undefined || own === "" ? "" : `#${own}`),
  );
  return found?.[1];
}

/** The browser's storage may refuse access (a private window, a policy): the token then lives in this tab only. */
export interface TokenStore {
  readonly saved: () => string | undefined;
  readonly save: (token: string) => void;
  readonly drop: (token: string) => void;
}

export function tokenStore(storage: () => Storage): TokenStore {
  let inTab: string | undefined;
  return {
    saved: () => {
      try {
        return storage().getItem(TOKEN_KEY) ?? undefined;
      } catch {
        return inTab;
      }
    },
    save: (token) => {
      inTab = token;
      try {
        storage().setItem(TOKEN_KEY, token);
      } catch {
        // The token stays in this tab.
      }
    },
    drop: (token) => {
      if (inTab === token) {
        inTab = undefined;
      }
      try {
        if (storage().getItem(TOKEN_KEY) === token) {
          storage().removeItem(TOKEN_KEY);
        }
      } catch {
        // Nothing was saved.
      }
    },
  };
}

/** `POST /session`: the token for a code. Same origin, no credentials, no referrer, no cache. */
export async function redeem(origin: string, code: string): Promise<string> {
  const answer = await fetch(new URL("/session", origin), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code }),
    cache: "no-store",
    credentials: "omit",
    referrerPolicy: "no-referrer",
  });
  if (answer.status !== 200) {
    throw new Error(`The server refused the code with status ${String(answer.status)}`);
  }
  // BOUNDARY: the server's answer to POST /session.
  const body: unknown = await answer.json();
  if (typeof body !== "object" || body === null || !("token" in body)) {
    throw new Error("The server's answer has no token");
  }
  const { token } = body;
  if (typeof token !== "string" || token.length === 0) {
    throw new Error("The server's answer has no token");
  }
  return token;
}
