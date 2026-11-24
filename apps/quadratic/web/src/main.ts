import "virtual:cssforge.css";
import "./page.css";
import { findHost } from "@acid-cat/core-effect/WebMcp";
import { createVaporApp } from "vue";
import { createActor } from "xstate";

import App from "./app.vue";
import { boardMachine } from "./board/board.machine.ts";
import { commandsMachine, type Command } from "./board/commands.machine.ts";
import { savedViewsMachine } from "./board/saved-views.machine.ts";
import { openQuadratic } from "./core/quadratic.ts";
import { Drafts } from "./edit/drafts.ts";
import { keysMachine } from "./keys/keys.machine.ts";
import { ALL, shiftPlan, shownGroups } from "./board/view.ts";
import { atPast, createRouter, paths, switched } from "./route.ts";
import { vaporComponent } from "./vapor.ts";
import { workspaceKey } from "./workspace.ts";

const router = createRouter(window);
const quadratic = await openQuadratic({
  origin: location.origin,
  modelContext: findHost(),
  writable: () => !atPast(router.route.value),
});

const board = createActor(boardMachine(quadratic));
const commands = createActor(
  commandsMachine({
    quadratic,
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
const SAVED_VIEWS = "quadratic.saved-views";
const savedViews = createActor(
  savedViewsMachine({
    read: () => {
      try {
        // BOUNDARY: what this browser stored; the store decodes it.
        return JSON.parse(localStorage.getItem(SAVED_VIEWS) ?? "null");
      } catch {
        return null;
      }
    },
    write: (stored) => {
      localStorage.setItem(SAVED_VIEWS, JSON.stringify(stored));
    },
  }),
);
window.addEventListener("storage", (event) => {
  if (event.key === SAVED_VIEWS) {
    try {
      savedViews.send({ type: "loaded", stored: JSON.parse(event.newValue ?? "null") });
    } catch {
      savedViews.send({ type: "loaded", stored: null });
    }
  }
});

board.start();
commands.start();
keys.start();
savedViews.start();

const app = createVaporApp(vaporComponent(App));
app.provide(workspaceKey, {
  quadratic,
  board,
  keys,
  commands,
  savedViews,
  router,
  drafts: new Drafts(),
});
app.mount("#app");
