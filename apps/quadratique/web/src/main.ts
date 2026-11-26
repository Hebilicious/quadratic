import "virtual:cssforge.css";
import "./page.css";
import { findHost } from "@acid-cat/core-effect/WebMcp";
import { createVaporApp } from "vue";
import { createActor } from "xstate";

import App from "./app.vue";
import { boardMachine } from "./board/board.machine.ts";
import { commandsMachine, type Command } from "./board/commands.machine.ts";
import { savedViewsMachine } from "./board/saved-views.machine.ts";
import { openQuadratique } from "./core/quadratique.ts";
import { Drafts } from "./edit/drafts.ts";
import { keysMachine } from "./keys/keys.machine.ts";
import { ALL, shiftPlan, shownGroups } from "./board/view.ts";
import { atPast, createRouter, paths, switched } from "./route.ts";
import { vaporComponent } from "./vapor.ts";
import { workspaceKey } from "./workspace.ts";
import { pairingMachine, type AppSession } from "./pairing/pairing.machine.ts";
import {
  NOT_PAIRED_AFTER,
  NOT_PAIRED_BEFORE,
  PAIR_COMMAND,
  TOKEN_KEY,
  redeem,
  takeCode,
  tokenStore,
} from "./pairing/pairing.ts";

// The one-time code leaves the address before anything else reads it.
const code = takeCode(window);
const router = createRouter(window);
const tokens = tokenStore(() => localStorage);
const root = document.getElementById("app") ?? document.body;

/** The app with the bearer `token`, mounted in `mount`; the function it resolves to ends it. */
async function startApp(
  token: () => string,
  unauthorized: () => void,
  mount: HTMLElement,
): Promise<() => Promise<void>> {
  const quadratique = await openQuadratique({
    origin: location.origin,
    access: { token, unauthorized },
    modelContext: findHost(),
    writable: () => !atPast(router.route.value),
  });

  const board = createActor(boardMachine(quadratique));
  const commands = createActor(
    commandsMachine({
      quadratique,
      current: (issue) => {
        const { issues, now } = board.getSnapshot().context;
        return issues[issue] ?? now[issue];
      },
      settle: (issues) => {
        board.send({ type: "settled", issues });
      },
    }),
  );
  const keys = createActor(
    keysMachine({
      goToWaiting: () => {
        router.go(paths.waiting());
      },
      goToNow: () => {
        router.go(paths.now());
      },
      goToHistory: () => {
        const route = router.route.value;
        router.go(
          paths.history(
            route.view === "list" || route.view === "board" || route.view === "history"
              ? route.scope
              : ALL,
          ),
        );
      },
      focusSearch: () => {
        document.getElementById("filter-text")?.focus();
      },
      open: (issue) => {
        router.go(paths.issue(issue));
      },
      switchView: () => {
        const other = switched(router.route.value);
        if (other !== undefined) {
          router.go(other);
        }
      },
      shift: (ids, by) => {
        const route = router.route.value;
        if (route.view !== "list" && route.view !== "board") {
          return;
        }
        const { projects, issues, milestones } = board.getSnapshot().context;
        const groups = shownGroups({
          grouping: route.query.group,
          filter: route.query.filter,
          scope: route.scope,
          projects,
          issues,
          milestones,
        });
        commands.send({
          type: "run",
          commands: ids.flatMap((id): Command[] => {
            const issue = issues[id];
            return issue === undefined
              ? []
              : [
                  {
                    kind: "shift",
                    issue: id,
                    by,
                    plan: shiftPlan(route.query.group, groups, issue, { projects, milestones }),
                  },
                ];
          }),
        });
      },
    }),
  );
  /** Where the browser kept saved views before the server kept them; read to import them, then removed. */
  const LEGACY_SAVED_VIEWS = "quadratique.saved-views";
  const savedViews = createActor(
    savedViewsMachine({
      ...quadratique.views,
      legacy: {
        read: () => {
          try {
            return localStorage.getItem(LEGACY_SAVED_VIEWS);
          } catch {
            return null;
          }
        },
        write: (value) => {
          localStorage.setItem(LEGACY_SAVED_VIEWS, value);
        },
        removeIf: (value) => {
          if (localStorage.getItem(LEGACY_SAVED_VIEWS) === value) {
            localStorage.removeItem(LEGACY_SAVED_VIEWS);
          }
        },
      },
      newSource: () => crypto.randomUUID(),
    }),
  );

  board.start();
  commands.start();
  keys.start();
  savedViews.start();

  const app = createVaporApp(vaporComponent(App));
  app.provide(workspaceKey, {
    quadratique,
    board,
    keys,
    commands,
    savedViews,
    router,
    drafts: new Drafts(),
  });
  app.mount(mount);
  return async () => {
    app.unmount();
    for (const actor of [savedViews, keys, commands, board]) {
      actor.stop();
    }
    await quadratique.dispose();
  };
}

/** Starts the app; ending it before it has started ends it as soon as it has. */
function openApp(token: string, unauthorized: () => void): AppSession {
  const mount = document.createElement("div");
  root.replaceChildren(mount);
  let current = token;
  let ended = false;
  let close: (() => Promise<void>) | undefined;
  void startApp(() => current, unauthorized, mount).then(
    (started) => {
      if (ended) {
        void started();
      } else {
        close = started;
      }
    },
    (error: unknown) => {
      reportError(error);
    },
  );
  return {
    end: () => {
      ended = true;
      void close?.();
      mount.remove();
    },
    use: (next) => {
      current = next;
    },
  };
}

function showNotPaired(): void {
  const sentence = document.createElement("p");
  const command = document.createElement("code");
  command.textContent = PAIR_COMMAND;
  sentence.append(NOT_PAIRED_BEFORE, command, NOT_PAIRED_AFTER);
  const page = document.createElement("main");
  page.className = "not-paired";
  page.append(sentence);
  root.replaceChildren(page);
}

const pairing = createActor(
  pairingMachine(
    {
      redeem: async (pairCode) => await redeem(location.origin, pairCode),
      save: tokens.save,
      drop: tokens.drop,
      open: openApp,
    },
    { code, saved: tokens.saved() },
  ),
);
pairing.subscribe((snapshot) => {
  if (snapshot.matches("unpaired") || snapshot.matches("refused")) {
    showNotPaired();
  }
});
// Another tab that pairs, or that drops a refused token, changes the saved token: this tab follows.
window.addEventListener("storage", (event) => {
  if (event.key !== TOKEN_KEY && event.key !== null) {
    return;
  }
  const token = tokens.saved();
  pairing.send(token === undefined ? { type: "cleared" } : { type: "stored", token });
});
// A link of quadratique ui opened in a tab that shows this address already changes only the fragment.
window.addEventListener("hashchange", () => {
  const next = takeCode(window);
  if (next !== undefined) {
    pairing.send({ type: "code", code: next });
  }
});
pairing.start();
