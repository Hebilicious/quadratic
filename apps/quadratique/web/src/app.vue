<script setup lang="ts" vapor>
import { computed, nextTick, onScopeDispose, watch, watchEffect } from "vue";

import { connectionPhase } from "./board/connection.machine.ts";
import { nowView, waitingCount } from "./board/now.ts";
import {
  columnsOf,
  isFiltered,
  movable,
  patchOfMove,
  projectScope,
  ALL,
  shownGroups,
  statusesOf,
  type Scope,
} from "./board/view.ts";
import { rolesOf } from "./board/issues.ts";
import { savedViewOf, type SavedView } from "./board/saved-views.machine.ts";
import { Refused } from "./core/quadratique.ts";
import { focusOf, keyLabel, shortcutOf } from "./keys/keys.ts";
import type { Moment, Surface } from "./keys/keys.machine.ts";
import { NOW_QUERY, paths, projectOfIssue, withQuery } from "./route.ts";
import { useSnapshot } from "./use-actor.ts";
import BoardView from "./views/board-view.vue";
import CommandMenu from "./views/command-menu.vue";
import CommandsReport from "./views/commands-report.vue";
import CreateDialog from "./views/create-dialog.vue";
import HelpDialog from "./views/help-dialog.vue";
import IssueDetail from "./views/issue-detail.vue";
import IssueList from "./views/issue-list.vue";
import HistoryView from "./views/history-view.vue";
import MomentBanner from "./views/moment-banner.vue";
import NowView from "./views/now-view.vue";
import SavedViewItem from "./views/saved-view-item.vue";
import { failureText } from "./views/present.ts";
import ViewBar from "./views/view-bar.vue";
import { useWorkspace } from "./workspace.ts";

const { board, keys, commands, savedViews, router } = useWorkspace();
const snapshot = useSnapshot(board);
const savedSnapshot = useSnapshot(savedViews);
const views = computed(() => savedSnapshot.value.context.views);
const importReport = computed(() => savedSnapshot.value.context.report);
const keysSnapshot = useSnapshot(keys);
const connectionActor = board.getSnapshot().children.connection;
if (connectionActor === undefined) {
  throw new Error("The board started without its live connection");
}
const connection = useSnapshot(connectionActor);

const phase = computed(() => connectionPhase(connection.value));
const route = router.route;
const projects = computed(() => snapshot.value.context.projects);
const issuesRoute = computed(() => {
  const current = route.value;
  return current.view === "list" || current.view === "board" ? current : undefined;
});
const scope = computed((): Scope | undefined => {
  const current = route.value;
  if (current.view === "list" || current.view === "board" || current.view === "history") {
    return current.scope;
  }
  return current.view === "issue" ? projectScope(projectOfIssue(current.issue)) : undefined;
});
const projectKey = computed(() => (scope.value?.kind === "project" ? scope.value.key : undefined));
const project = computed(() => projects.value.find((each) => each.key === projectKey.value));
const scopeKnown = computed(() => scope.value?.kind === "all" || project.value !== undefined);
const query = computed(() => issuesRoute.value?.query ?? NOW_QUERY);
const moment = computed((): Moment => (query.value.at === undefined ? "now" : "past"));
const projectsState = computed(() =>
  snapshot.value.matches({ projects: "failed" })
    ? "failed"
    : snapshot.value.matches({ projects: "ready" })
      ? "ready"
      : "loading",
);
const issuesState = computed(() => {
  const value = snapshot.value;
  if (value.matches({ issues: "failed" })) {
    return "failed";
  }
  return value.matches({ issues: "ready" }) || value.matches({ issues: "past" })
    ? "ready"
    : "loading";
});
const issuesFailure = computed(() =>
  snapshot.value.context.issuesError instanceof Refused
    ? failureText(snapshot.value.context.issuesError)
    : "The issues did not load.",
);
const nowState = computed(() =>
  snapshot.value.matches({ now: "failed" })
    ? "failed"
    : snapshot.value.matches({ now: "ready" })
      ? "ready"
      : "loading",
);
const groups = computed(() => {
  const current = issuesRoute.value;
  if (current === undefined || !scopeKnown.value) {
    return [];
  }
  const { issues, milestones } = snapshot.value.context;
  return shownGroups({
    grouping: current.query.group,
    filter: current.query.filter,
    scope: current.scope,
    projects: projects.value,
    issues,
    milestones,
  });
});
const milestonesState = computed(() => {
  if (query.value.group !== "milestone") {
    return "unused";
  }
  if (snapshot.value.matches({ milestones: { on: "failed" } })) {
    return "failed";
  }
  return snapshot.value.matches({ milestones: { on: "read" } }) ? "read" : "reading";
});
const title = computed(() => {
  const current = route.value;
  if (current.view === "now") {
    return "Now";
  }
  if (current.view === "issue") {
    return current.issue;
  }
  const name =
    scope.value?.kind === "all" ? "All projects" : (project.value?.name ?? "Quadratique");
  return current.view === "history" ? `${name}: history` : name;
});
const scopeStatuses = computed(() =>
  scope.value === undefined ? [] : statusesOf(projects.value, scope.value),
);
const scopeLabels = computed(() =>
  [
    ...new Set(Object.values(snapshot.value.context.issues).flatMap((issue) => issue.labels)),
  ].toSorted(),
);
const scopeRoles = computed(() => rolesOf(snapshot.value.context.issues));

function viewPath(view: SavedView): string {
  const current = issuesRoute.value;
  const query = { ...(current?.query ?? NOW_QUERY), filter: view.filter };
  return current?.view === "board" ? paths.board(view.scope, query) : paths.list(view.scope, query);
}

function isCurrentView(view: SavedView): boolean {
  const current = issuesRoute.value;
  return current !== undefined && savedViewOf([view], current.scope, current.query.filter) === view;
}

const now = computed(() => nowView(snapshot.value.context.now, projects.value));
const waiting = computed(() => waitingCount(now.value));

const surface = computed((): Surface => {
  const current = issuesRoute.value;
  if (current === undefined || !scopeKnown.value) {
    return "other";
  }
  return current.view;
});
const shownGroupsOfList = computed(() => groups.value.filter((group) => group.issues.length > 0));
const draggable = computed(() => moment.value === "now" && movable(query.value.group));
const emptyText = computed(() => {
  if (moment.value === "past") {
    return "No issue existed then.";
  }
  if (isFiltered(query.value.filter)) {
    return "No issue matches the filters.";
  }
  return "No issue yet. Press C to create one.";
});
const layout = computed(() => {
  if (surface.value === "board") {
    return columnsOf(groups.value);
  }
  return surface.value === "list" ? [columnsOf(shownGroupsOfList.value).flat()] : [];
});

const cursor = computed(() => keysSnapshot.value.context.cursor);
const selected = computed(() => keysSnapshot.value.context.selected);
const peeking = computed(() => keysSnapshot.value.matches({ peek: "open" }));
const overlay = computed(() => {
  const value = keysSnapshot.value;
  if (value.matches({ overlay: "menu" })) {
    return "menu";
  }
  if (value.matches({ overlay: "help" })) {
    return "help";
  }
  return value.matches({ overlay: "create" }) ? "create" : "none";
});
const openIssue = computed(() => {
  const current = route.value;
  if (current.view === "issue") {
    return current.issue;
  }
  return peeking.value ? cursor.value : undefined;
});
const detail = computed(() =>
  snapshot.value.matches({ detail: "open" }) ? snapshot.value.children.detail : undefined,
);
const opened = computed(() => {
  if (detail.value === undefined) {
    return [];
  }
  const id: string = detail.value.getSnapshot().context.id;
  return [{ id, actor: detail.value }];
});

watchEffect(() => {
  const current = route.value;
  board.send({
    type: "select",
    scope: current.view === "history" || current.view === "now" ? undefined : scope.value,
    at: issuesRoute.value?.query.at,
  });
});

watchEffect(() => {
  const current = route.value;
  board.send({
    type: "history",
    scope: current.view === "history" && scopeKnown.value ? current.scope : undefined,
  });
});

watchEffect(() => {
  board.send({
    type: "milestones",
    wanted: surface.value !== "other" && query.value.group === "milestone",
  });
});

watchEffect(() => {
  keys.send({ type: "layout", surface: surface.value, moment: moment.value, layout: layout.value });
});

watch(moment, (now) => {
  if (now === "past") {
    commands.send({ type: "past" });
  }
});

watchEffect(() => {
  board.send({ type: "open", issue: openIssue.value });
});

watchEffect(() => {
  const first = projects.value[0];
  if (route.value.view === "home" && first !== undefined) {
    router.replace(paths.list(projectScope(first.key)));
  }
});

watch([cursor, layout, overlay], async ([issue, , open]) => {
  await nextTick();
  const active = document.activeElement;
  const onTheIssues =
    active === null ||
    active === document.body ||
    (active instanceof HTMLElement && active.dataset["issue"] !== undefined);
  if (issue === undefined || open !== "none" || !onTheIssues) {
    return;
  }
  const element = document.querySelector<HTMLElement>(`[data-issue="${CSS.escape(issue)}"]`);
  if (element !== null && element !== active) {
    element.focus();
  }
});

function onKey(event: KeyboardEvent): void {
  if (event.defaultPrevented || event.isComposing) {
    return;
  }
  const key = shortcutOf(event, focusOf(document.activeElement));
  if (key === undefined) {
    return;
  }
  event.preventDefault();
  keys.send({ type: "key", key });
}
window.addEventListener("keydown", onKey);
onScopeDispose(() => {
  window.removeEventListener("keydown", onKey);
});

function follow(event: MouseEvent): void {
  const link = event.currentTarget;
  if (
    !(link instanceof HTMLAnchorElement) ||
    event.button !== 0 ||
    event.metaKey ||
    event.ctrlKey ||
    event.shiftKey ||
    event.altKey
  ) {
    return;
  }
  event.preventDefault();
  router.go(link.pathname + link.search + link.hash);
}

function move({ issue, group }: { readonly issue: string; readonly group: string }): void {
  const summary = snapshot.value.context.issues[issue];
  const target = groups.value.find((each) => each.key === group)?.move;
  if (summary === undefined || target === undefined) {
    return;
  }
  const patch = patchOfMove(target, summary, {
    projects: projects.value,
    milestones: snapshot.value.context.milestones,
  });
  if (patch !== undefined) {
    commands.send({
      type: "run",
      commands: [{ kind: "update", issue, revision: summary.revision, patch }],
    });
  }
}

function retry(): void {
  board.send({ type: "retry" });
}

function closePeek(): void {
  keys.send({ type: "peek" });
}

const phaseLabel = {
  connecting: "Connecting",
  live: "Live",
  reconnecting: "Reconnecting",
} as const;
</script>

<template>
  <div class="shell">
    <nav class="sidebar" aria-label="Places" :inert="overlay === 'none' ? undefined : true">
      <p class="brand">Quadratique</p>
      <a
        class="nav-link"
        :href="paths.now()"
        :aria-current="route.view === 'now' ? 'page' : undefined"
        @click="follow"
      >
        <span>Now</span>
        <span v-if="waiting > 0" class="nav-count" :aria-label="`${waiting} wait on you`">{{ waiting }}</span>
      </a>
      <a
        class="nav-link"
        :href="paths.history(ALL)"
        :aria-current="route.view === 'history' && scope?.kind === 'all' ? 'page' : undefined"
        @click="follow"
      >
        <span>History</span>
      </a>
      <p class="nav-heading">Projects</p>
      <p v-if="projectsState === 'failed'" class="nav-note">
        The projects did not load. <button class="link-button" type="button" @click="retry">Retry</button>
      </p>
      <p v-else-if="projectsState === 'ready' && projects.length === 0" class="nav-note">
        No project yet. Run <code>quadratique project create</code> in a repository.
      </p>
      <a
        v-if="projects.length > 0"
        class="nav-link"
        :href="paths.list(ALL)"
        data-named="true"
        :aria-current="issuesRoute?.scope.kind === 'all' ? 'page' : undefined"
        @click="follow"
      >
        <span class="nav-key">∗</span>
        <span class="nav-name">All projects</span>
      </a>
      <a
        v-for="each in projects"
        :key="each.key"
        class="nav-link"
        :href="paths.list(projectScope(each.key))"
        :aria-current="each.key === projectKey && issuesRoute !== undefined ? 'page' : undefined"
        @click="follow"
      >
        <span class="nav-key">{{ each.key }}</span>
        <span class="nav-name">{{ each.name }}</span>
      </a>
      <template v-if="views.length > 0">
        <p class="nav-heading">Views</p>
        <SavedViewItem
          v-for="view in views"
          :key="view.id"
          :view="view"
          :views="views"
          :href="viewPath(view)"
          :current="isCurrentView(view)"
          @follow="follow"
        />
      </template>
      <section
        v-if="importReport.shortened.length > 0 ||
          importReport.kept.length > 0 ||
          importReport.interrupted"
        class="nav-note"
        role="status"
        aria-label="Views from this browser"
      >
        <p v-for="each in importReport.shortened" :key="`cut:${each.from}`">
          The view {{ each.from }} from this browser is saved as {{ each.to }}: a name has at most
          100 characters.
        </p>
        <p v-for="each in importReport.kept" :key="`kept:${each.name}`">
          The view {{ each.name }} stays in this browser, not imported: {{ each.reason }}.
        </p>
        <p v-if="importReport.interrupted">
          The views of this browser are not all imported yet: the server did not answer. The next
          page goes on.
        </p>
      </section>
      <div class="sidebar-foot">
        <button
          class="ghost-button"
          type="button"
          @click="keys.send({ type: 'openMenu', query: '' })"
        >
          Commands <kbd>{{ keyLabel("Mod K") }}</kbd>
        </button>
        <button class="ghost-button" type="button" @click="keys.send({ type: 'openHelp' })">
          Shortcuts <kbd>?</kbd>
        </button>
      </div>
    </nav>

    <main class="main" :inert="overlay === 'none' ? undefined : true">
      <header class="topbar">
        <h1 class="title">{{ title }}</h1>
        <div
          v-if="scope !== undefined && scopeKnown && route.view !== 'issue'"
          class="switch"
          role="group"
          aria-label="View"
        >
          <a
            class="switch-option"
            :href="paths.list(scope, query)"
            :aria-current="route.view === 'list' ? 'page' : undefined"
            @click="follow"
          >List</a>
          <a
            class="switch-option"
            :href="paths.board(scope, query)"
            :aria-current="route.view === 'board' ? 'page' : undefined"
            @click="follow"
          >Board</a>
          <a
            class="switch-option"
            :href="paths.history(scope)"
            :aria-current="route.view === 'history' ? 'page' : undefined"
            @click="follow"
          >History</a>
        </div>
        <button
          v-if="projects.length > 0 && moment === 'now'"
          class="create-button"
          type="button"
          @click="keys.send({ type: 'openCreate' })"
        >New issue <kbd>C</kbd></button>
        <p v-if="moment === 'past'" class="connection" data-phase="past" role="status">The past</p>
        <p v-else class="connection" :data-phase="phase" role="status">{{ phaseLabel[phase] }}</p>
      </header>

      <div class="workspace" :data-peeking="peeking && surface !== 'other'">
        <div v-if="issuesRoute !== undefined && scopeKnown" class="content" data-bar="true">
          <MomentBanner
            v-if="issuesRoute.query.at !== undefined"
            :state="issuesState"
            :moment="snapshot.context.moment"
            :asked="issuesRoute.query.at"
            :now-href="withQuery(issuesRoute, { ...issuesRoute.query, at: undefined })"
            :history-href="paths.history(issuesRoute.scope)"
            @follow="follow"
          />
          <ViewBar
            :route="issuesRoute"
            :statuses="scopeStatuses"
            :labels="scopeLabels"
            :roles="scopeRoles"
            :views="views"
          />
          <p v-if="milestonesState === 'reading'" class="bar-note" aria-busy="true">Reading the milestones…</p>
          <p v-else-if="milestonesState === 'failed'" class="bar-note" role="alert">
            The milestones did not load. <button class="link-button" type="button" @click="retry">Retry</button>
          </p>
          <div class="issues">
            <IssueList
              v-if="issuesRoute.view === 'list'"
              :groups="shownGroupsOfList"
              :state="issuesState"
              :failure="issuesFailure"
              :empty="emptyText"
              :cursor="cursor"
              :selected="selected"
              @retry="retry"
            />
            <BoardView
              v-else
              :groups="groups"
              :state="issuesState"
              :failure="issuesFailure"
              :draggable="draggable"
              :cursor="cursor"
              :selected="selected"
              @retry="retry"
              @move="move"
            />
          </div>
        </div>
        <div v-else class="content">
          <HistoryView
            v-if="route.view === 'history' && scopeKnown"
            :scope="route.scope"
            :projects="projects"
          />
          <template v-else-if="route.view === 'issue'">
            <IssueDetail
              v-for="each in opened"
              :key="each.id"
              :detail="each.actor"
              :closing="route.closing"
              mode="page"
            />
          </template>
          <NowView
            v-else-if="route.view === 'now'"
            :now="now"
            :projects="projects"
            :state="nowState"
            :section="route.section"
            @retry="retry"
          />
          <p v-else-if="route.view === 'missing'" class="empty">There is no page at this address.</p>
          <p
            v-else-if="scope?.kind === 'project' && !scopeKnown && projectsState === 'ready'"
            class="empty"
          >
            There is no project {{ projectKey }}.
          </p>
        </div>
        <aside
          v-if="peeking && surface !== 'other' && detail !== undefined && openIssue !== undefined"
          class="peek"
          aria-label="Peek"
        >
          <div class="peek-bar">
            <a class="peek-open" :href="paths.issue(openIssue)" @click="follow">Open {{ openIssue }} <kbd>↵</kbd></a>
            <button class="ghost-button" type="button" aria-label="Close the peek" @click="closePeek">
              Close <kbd>Esc</kbd>
            </button>
          </div>
          <IssueDetail
            v-for="each in opened"
            :key="each.id"
            :detail="each.actor"
            :closing="false"
            mode="peek"
          />
        </aside>
      </div>
    </main>

    <CommandMenu v-if="overlay === 'menu'" :surface="surface" :moment="moment" />
    <CreateDialog v-if="overlay === 'create'" :project="project" />
    <HelpDialog v-if="overlay === 'help'" />
    <CommandsReport />
  </div>
</template>
