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

const router = createRouter(window);
const quadratique = await openQuadratique({
  origin: location.origin,
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
/** Where the browser kept saved views before the server kept them; read once to import them. */
const LEGACY_SAVED_VIEWS = "quadratique.saved-views";
const savedViews = createActor(
  savedViewsMachine({
    ...quadratique.views,
    legacy: {
      read: () => {
        try {
          // BOUNDARY: what this browser stored; the machine decodes it.
          return JSON.parse(localStorage.getItem(LEGACY_SAVED_VIEWS) ?? "null");
        } catch {
          return null;
        }
      },
      clear: () => {
        try {
          localStorage.removeItem(LEGACY_SAVED_VIEWS);
        } catch {
          // A browser that keeps no site data kept no views either.
        }
      },
    },
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
app.mount("#app");
