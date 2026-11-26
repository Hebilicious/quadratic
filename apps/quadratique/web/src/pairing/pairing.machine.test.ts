import { describe, expect, test } from "vitest";
import { createActor, waitFor } from "xstate";

import { HANG_GUARD } from "../test/fixtures.ts";
import { pairingMachine, type PairingStart } from "./pairing.machine.ts";
import { takeCode, tokenStore, TOKEN_KEY } from "./pairing.ts";

class Ports {
  saved: string | undefined;
  readonly opened: string[] = [];
  readonly ended: string[] = [];
  readonly redeemed: string[] = [];
  /** The tokens the running app was told to use for its later requests. */
  readonly used: string[] = [];
  unauthorized: (() => void) | undefined;
  refuse = false;
  /** While set, a redemption waits until the test settles it. */
  hold = false;
  readonly waiting: { code: string; settle: (refused: boolean) => void }[] = [];

  readonly ports = {
    redeem: async (code: string) => {
      this.redeemed.push(code);
      if (this.hold) {
        const refused = await new Promise<boolean>((settle) => {
          this.waiting.push({ code, settle });
        });
        if (refused) {
          throw new Error("refused");
        }
        return `token-for-${code}`;
      }
      if (this.refuse) {
        throw new Error("refused");
      }
      return `token-for-${code}`;
    },
    save: (token: string) => {
      this.saved = token;
    },
    drop: (token: string) => {
      if (this.saved === token) {
        this.saved = undefined;
      }
    },
    open: (token: string, unauthorized: () => void) => {
      this.opened.push(token);
      this.unauthorized = unauthorized;
      return {
        end: () => {
          this.ended.push(token);
        },
        use: (next: string) => {
          this.used.push(next);
        },
      };
    },
  };
}

function started(ports: Ports, start: PairingStart) {
  const actor = createActor(pairingMachine(ports.ports, start));
  actor.start();
  return actor;
}

describe("the page's pairing", () => {
  test("a code from quadratique ui is redeemed, the token saved, and the app opened with it", async () => {
    const ports = new Ports();

    const actor = started(ports, { code: "abc", saved: undefined });

    await waitFor(actor, (snapshot) => snapshot.matches("paired"), HANG_GUARD);
    expect(ports.redeemed).toEqual(["abc"]);
    expect(ports.saved).toBe("token-for-abc");
    expect(ports.opened).toEqual(["token-for-abc"]);
  });

  test("a saved token opens the app at once, and no token shows the sentence", () => {
    const ports = new Ports();

    const paired = started(ports, { code: undefined, saved: "kept" });
    const unpaired = started(new Ports(), { code: undefined, saved: undefined });

    expect(paired.getSnapshot().matches("paired")).toBe(true);
    expect(ports.opened).toEqual(["kept"]);
    expect(unpaired.getSnapshot().matches("unpaired")).toBe(true);
  });

  test("a used or expired code is refused, unless a saved token still opens the app", async () => {
    const refusedPorts = new Ports();
    refusedPorts.refuse = true;
    const keptPorts = new Ports();
    keptPorts.refuse = true;

    const refused = started(refusedPorts, { code: "old", saved: undefined });
    const kept = started(keptPorts, { code: "old", saved: "kept" });

    await waitFor(refused, (snapshot) => snapshot.matches("refused"), HANG_GUARD);
    await waitFor(kept, (snapshot) => snapshot.matches("paired"), HANG_GUARD);
    expect(refusedPorts.opened).toEqual([]);
    expect(keptPorts.opened).toEqual(["kept"]);
  });

  test("a 401 ends the app and drops the token, and a token another tab saves opens it again", () => {
    const ports = new Ports();
    ports.saved = "kept";
    const actor = started(ports, { code: undefined, saved: "kept" });

    ports.unauthorized?.();

    expect(actor.getSnapshot().matches("unpaired")).toBe(true);
    expect(ports.ended).toEqual(["kept"]);
    expect(ports.saved).toBeUndefined();
    actor.send({ type: "stored", token: "fresh" });
    expect(actor.getSnapshot().matches("paired")).toBe(true);
    expect(ports.opened).toEqual(["kept", "fresh"]);
  });

  test("a paired tab follows another tab: a new token rebuilds the app, a cleared one ends it", () => {
    const ports = new Ports();
    const actor = started(ports, { code: undefined, saved: "kept" });

    actor.send({ type: "stored", token: "kept" });
    actor.send({ type: "stored", token: "fresh" });
    actor.send({ type: "cleared" });

    expect(ports.opened).toEqual(["kept", "fresh"]);
    expect(ports.ended).toEqual(["kept", "fresh"]);
    expect(actor.getSnapshot().matches("unpaired")).toBe(true);
  });

  test("a 401 from an app that already ended changes nothing", () => {
    const ports = new Ports();
    const actor = started(ports, { code: undefined, saved: "kept" });
    const stale = ports.unauthorized;
    actor.send({ type: "stored", token: "fresh" });

    stale?.();

    expect(actor.getSnapshot().matches("paired")).toBe(true);
    expect(ports.ended).toEqual(["kept"]);
  });
});

test("a code that arrives in an open tab is redeemed there, and the app reopens with the token", async () => {
  const ports = new Ports();
  const actor = started(ports, { code: undefined, saved: undefined });

  actor.send({ type: "code", code: "later" });

  await waitFor(actor, (snapshot) => snapshot.matches("paired"), HANG_GUARD);
  expect(ports.opened).toEqual(["token-for-later"]);
});

describe("a code that arrives in a paired tab", () => {
  test("is exchanged while the app keeps running, and a token it already has changes nothing", async () => {
    const ports = new Ports();
    ports.saved = "token-for-again";
    const actor = started(ports, { code: undefined, saved: "token-for-again" });

    actor.send({ type: "code", code: "again" });

    await waitFor(actor, () => ports.redeemed.length === 1, HANG_GUARD);
    await waitFor(actor, (snapshot) => snapshot.matches({ paired: "running" }), HANG_GUARD);
    expect(ports.opened).toEqual(["token-for-again"]);
    expect(ports.ended).toEqual([]);
  });

  test("that is refused, a fake one included, changes nothing", async () => {
    const ports = new Ports();
    ports.refuse = true;
    const actor = started(ports, { code: undefined, saved: "kept" });

    actor.send({ type: "code", code: "fake" });

    await waitFor(actor, () => ports.redeemed.length === 1, HANG_GUARD);
    await waitFor(actor, (snapshot) => snapshot.matches({ paired: "running" }), HANG_GUARD);
    expect(ports.opened).toEqual(["kept"]);
    expect(ports.ended).toEqual([]);
    expect(ports.saved).toBeUndefined();
  });

  test("that gives a new token saves it, and the running app uses it for its later requests without restarting", async () => {
    const ports = new Ports();
    const actor = started(ports, { code: undefined, saved: "old" });

    actor.send({ type: "code", code: "new" });

    await waitFor(actor, () => ports.saved === "token-for-new", HANG_GUARD);
    await waitFor(actor, (snapshot) => snapshot.matches({ paired: "running" }), HANG_GUARD);
    expect(ports.used).toEqual(["token-for-new"]);
    expect(ports.ended).toEqual([]);
    expect(ports.opened).toEqual(["old"]);
  });

  test("whose exchange outlives a 401 of the old token is kept, and the app goes on with the new token", async () => {
    const ports = new Ports();
    ports.saved = "old";
    ports.hold = true;
    const actor = started(ports, { code: undefined, saved: "old" });
    actor.send({ type: "code", code: "fresh" });
    await waitFor(actor, () => ports.waiting.length === 1, HANG_GUARD);

    ports.unauthorized?.();
    expect(actor.getSnapshot().matches({ paired: { exchanging: "refused" } })).toBe(true);
    expect(ports.saved).toBeUndefined();
    ports.waiting[0]?.settle(false);

    await waitFor(actor, (snapshot) => snapshot.matches({ paired: "running" }), HANG_GUARD);
    expect(ports.saved).toBe("token-for-fresh");
    expect(ports.used).toEqual(["token-for-fresh"]);
    expect(ports.ended).toEqual([]);
  });

  test("whose exchange fails after the old token was refused or cleared leaves the tab unpaired", async () => {
    for (const loss of ["unauthorized", "cleared"] as const) {
      const ports = new Ports();
      ports.hold = true;
      const actor = started(ports, { code: undefined, saved: "old" });
      actor.send({ type: "code", code: "fake" });
      await waitFor(actor, () => ports.waiting.length === 1, HANG_GUARD);

      if (loss === "unauthorized") {
        ports.unauthorized?.();
      } else {
        actor.send({ type: "cleared" });
      }
      ports.waiting[0]?.settle(true);

      await waitFor(actor, (snapshot) => snapshot.matches("unpaired"), HANG_GUARD);
      expect(ports.ended).toEqual(["old"]);
    }
  });
});

test("a second code that arrives while the first is redeemed replaces it", async () => {
  const ports = new Ports();
  ports.hold = true;
  const actor = started(ports, { code: "first", saved: undefined });
  await waitFor(actor, () => ports.waiting.length === 1, HANG_GUARD);

  actor.send({ type: "code", code: "second" });
  await waitFor(actor, () => ports.waiting.length === 2, HANG_GUARD);
  ports.waiting[0]?.settle(false);
  ports.waiting[1]?.settle(false);

  await waitFor(actor, (snapshot) => snapshot.matches("paired"), HANG_GUARD);
  expect(ports.redeemed).toEqual(["first", "second"]);
  expect(ports.opened).toEqual(["token-for-second"]);
  expect(ports.saved).toBe("token-for-second");
});

describe("the code in the address", () => {
  test("may carry the page's own fragment after it, which stays in the address", () => {
    const replaced: string[] = [];
    const view = {
      location: { hash: "#pair=abc&finish", pathname: "/issues/DEM-1", search: "" },
      history: {
        state: null,
        replaceState: (_state: unknown, _unused: string, url?: string | URL | null) => {
          replaced.push(String(url));
        },
      },
    };

    expect(takeCode(view)).toBe("abc");
    expect(replaced).toEqual(["/issues/DEM-1#finish"]);
  });

  test("is read and cleared from the address, keeping the path and the query", () => {
    const replaced: string[] = [];
    const view = {
      location: { hash: "#pair=abc_DEF-1", pathname: "/issues/DEM-1", search: "?x=1" },
      history: {
        state: null,
        replaceState: (_state: unknown, _unused: string, url?: string | URL | null) => {
          replaced.push(String(url));
        },
      },
    };

    expect(takeCode(view)).toBe("abc_DEF-1");
    expect(replaced).toEqual(["/issues/DEM-1?x=1"]);
  });

  test("another fragment is left alone, and a malformed code is cleared but not used", () => {
    const replaced: string[] = [];
    const history = {
      state: null,
      replaceState: (_state: unknown, _unused: string, url?: string | URL | null) => {
        replaced.push(String(url));
      },
    };

    expect(
      takeCode({ location: { hash: "#waiting", pathname: "/now", search: "" }, history }),
    ).toBe(undefined);
    expect(
      takeCode({ location: { hash: "#pair=<script>", pathname: "/", search: "" }, history }),
    ).toBe(undefined);
    expect(replaced).toEqual(["/"]);
  });
});

describe("the saved token", () => {
  test("is dropped only while it is still the one saved", () => {
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => {
        values.set(key, value);
      },
      removeItem: (key: string) => {
        values.delete(key);
      },
    } as unknown as Storage;
    const tokens = tokenStore(() => storage);

    tokens.save("old");
    values.set(TOKEN_KEY, "newer");
    tokens.drop("old");

    expect(tokens.saved()).toBe("newer");
  });

  test("lives in the tab when the browser refuses its storage", () => {
    const tokens = tokenStore(() => {
      throw new Error("denied");
    });

    tokens.save("in-tab");

    expect(tokens.saved()).toBe("in-tab");
  });
});
