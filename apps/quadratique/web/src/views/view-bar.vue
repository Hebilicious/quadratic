<script setup lang="ts" vapor>
import { computed, nextTick, ref, watchEffect } from "vue";

import type { Category, WorkflowStatus } from "../board/model.ts";
import {
  nameProblem,
  savedViewOf,
  writeState,
  type SavedView,
} from "../board/saved-views.machine.ts";
import {
  GROUPINGS,
  isFiltered,
  isGrouping,
  NO_FILTER,
  PRIORITY_ORDER,
  type AssigneeFilter,
  type Filter,
} from "../board/view.ts";
import { GROUPING_NAMES } from "../keys/menu.ts";
import { firstCharacters, LIMITS, withQuery, type Route } from "../route.ts";
import { useSnapshot } from "../use-actor.ts";
import { useWorkspace } from "../workspace.ts";
import { priorityLabel } from "./present.ts";

const props = defineProps<{
  readonly route: Extract<Route, { view: "list" | "board" }>;
  readonly statuses: readonly WorkflowStatus[];
  readonly labels: readonly string[];
  readonly roles: readonly string[];
  readonly views: readonly SavedView[];
}>();
const { router, savedViews } = useWorkspace();
const savedSnapshot = useSnapshot(savedViews);

const CATEGORIES: readonly { readonly category: Category; readonly name: string }[] = [
  { category: "backlog", name: "Backlog" },
  { category: "unstarted", name: "Not started" },
  { category: "started", name: "Started" },
  { category: "completed", name: "Completed" },
  { category: "canceled", name: "Canceled" },
];

const filter = computed(() => props.route.query.filter);
const filtered = computed(() => isFiltered(filter.value));
const statusCount = computed(() => filter.value.statuses.length + filter.value.categories.length);
const statusChoices = computed(
  (): readonly { key: string; name: string; category?: Category }[] => [
    ...props.statuses,
    ...filter.value.statuses
      .filter((key) => !props.statuses.some((status) => status.key === key))
      .map((key) => ({ key, name: key })),
  ],
);
const statusesFull = computed(() => filter.value.statuses.length >= LIMITS.statuses);
const labelsFull = computed(() => filter.value.labels.length >= LIMITS.labels);
const labelChoices = computed(() =>
  [...new Set([...props.labels, ...filter.value.labels])].toSorted(),
);

function setFilter(next: Filter): void {
  router.replace(withQuery(props.route, { ...props.route.query, filter: next }));
}

function toggled<T>(values: readonly T[], value: T): readonly T[] {
  return values.includes(value) ? values.filter((each) => each !== value) : [...values, value];
}

function group(event: Event): void {
  if (event.target instanceof HTMLSelectElement && isGrouping(event.target.value)) {
    router.go(withQuery(props.route, { ...props.route.query, group: event.target.value }));
  }
}

const assigneeValue = (assignee: AssigneeFilter | undefined): string =>
  assignee === undefined ? "" : assignee.kind === "role" ? `role:${assignee.role}` : assignee.kind;

const assigneeRoles = computed(() => {
  const chosen = filter.value.assignee;
  return chosen?.kind === "role" && !props.roles.includes(chosen.role)
    ? [...props.roles, chosen.role]
    : props.roles;
});

function assignee(event: Event): void {
  if (!(event.target instanceof HTMLSelectElement)) {
    return;
  }
  const value = event.target.value;
  const role = value.startsWith("role:") ? value.slice("role:".length) : undefined;
  setFilter({
    ...filter.value,
    assignee:
      value === "nobody" || value === "human"
        ? { kind: value }
        : role === undefined
          ? undefined
          : { kind: "role", role },
  });
}

function claim(event: Event): void {
  if (!(event.target instanceof HTMLSelectElement)) {
    return;
  }
  const value = event.target.value;
  setFilter({
    ...filter.value,
    claim: value === "mine" || value === "any" || value === "none" ? value : undefined,
  });
}

function ready(event: Event): void {
  if (event.target instanceof HTMLSelectElement) {
    const value = event.target.value;
    setFilter({ ...filter.value, ready: value === "" ? undefined : value === "true" });
  }
}

function search(event: Event): void {
  if (event.target instanceof HTMLInputElement) {
    const field = event.target;
    const text = firstCharacters(field.value, LIMITS.text);
    if (text !== field.value) {
      field.value = text;
    }
    setFilter({ ...filter.value, text });
  }
}

function leaveSearch(event: KeyboardEvent): void {
  if (event.key === "Escape" && event.target instanceof HTMLInputElement) {
    event.preventDefault();
    event.target.blur();
  }
}

type Saving = { readonly kind: "closed" } | { readonly kind: "naming"; readonly name: string };
const SAVE_FORM = "save";
const saving = ref<Saving>({ kind: "closed" });
const saveProblem = computed(() =>
  saving.value.kind === "naming" && saving.value.name.trim().length === 0
    ? nameProblem(props.views, "", undefined)
    : undefined,
);
const replacing = computed(() => {
  const current = saving.value;
  return current.kind === "naming"
    ? props.views.find(
        (view) => view.name.trim().toLowerCase() === current.name.trim().toLowerCase(),
      )
    : undefined;
});

const saveState = computed(() => writeState(savedSnapshot.value, SAVE_FORM));
const notSaved = computed(() => saving.value.kind === "naming" && saveState.value === "refused");
const submitted = ref(false);
watchEffect(() => {
  if (!submitted.value || saveState.value === "pending") {
    return;
  }
  submitted.value = false;
  if (saveState.value === "idle") {
    void closeSaving();
  }
});

async function startSaving(): Promise<void> {
  saving.value = {
    kind: "naming",
    name: savedViewOf(props.views, props.route.scope, filter.value)?.name ?? "",
  };
  await nextTick();
  document.getElementById("view-name")?.focus();
}

function save(event: Event): void {
  event.preventDefault();
  if (saving.value.kind !== "naming" || saveProblem.value !== undefined) {
    return;
  }
  savedViews.send({
    type: "save",
    form: SAVE_FORM,
    name: saving.value.name,
    scope: props.route.scope,
    filter: filter.value,
  });
  submitted.value = true;
}

async function closeSaving(): Promise<void> {
  savedViews.send({ type: "forget", form: SAVE_FORM });
  saving.value = { kind: "closed" };
  await nextTick();
  document.getElementById("save-view")?.focus();
}

function nameInput(event: Event): void {
  if (event.target instanceof HTMLInputElement) {
    savedViews.send({ type: "forget", form: SAVE_FORM });
    saving.value = { kind: "naming", name: event.target.value };
  }
}

function cancelSaving(event: KeyboardEvent): void {
  if (event.key === "Escape") {
    event.preventDefault();
    void closeSaving();
  }
}
</script>

<template>
  <div class="view-bar" role="toolbar" aria-label="How the issues are shown">
    <label class="bar-field">
      <span class="bar-label">Group by</span>
      <select class="bar-select" :value="route.query.group" @change="group">
        <option v-for="grouping in GROUPINGS" :key="grouping" :value="grouping">{{ GROUPING_NAMES[grouping] }}</option>
      </select>
    </label>
    <span class="bar-divider" aria-hidden="true"></span>
    <details class="bar-menu" :data-active="statusCount > 0">
      <summary>Status<span v-if="statusCount > 0" class="bar-count">{{ statusCount }}</span></summary>
      <fieldset class="bar-panel" aria-label="Status">
        <p class="bar-panel-heading">Category</p>
        <label v-for="each in CATEGORIES" :key="each.category" class="bar-check">
          <input
            type="checkbox"
            :checked="filter.categories.includes(each.category)"
            @change="setFilter({ ...filter, categories: toggled(filter.categories, each.category) })"
          />
          <span class="status-icon" :data-category="each.category" aria-hidden="true"></span>
          {{ each.name }}
        </label>
        <p class="bar-panel-heading">Status</p>
        <p v-if="statusesFull" id="status-cap" class="bar-panel-note">
          At most {{ LIMITS.statuses }} statuses at once: clear one to choose another.
        </p>
        <label v-for="status in statusChoices" :key="status.key" class="bar-check">
          <input
            type="checkbox"
            :checked="filter.statuses.includes(status.key)"
            :disabled="statusesFull && !filter.statuses.includes(status.key)"
            :aria-describedby="statusesFull ? 'status-cap' : undefined"
            @change="setFilter({ ...filter, statuses: toggled(filter.statuses, status.key) })"
          />
          <span class="status-icon" :data-category="status.category" aria-hidden="true"></span>
          {{ status.name }}
        </label>
      </fieldset>
    </details>
    <details class="bar-menu" :data-active="filter.priorities.length > 0">
      <summary>Priority<span v-if="filter.priorities.length > 0" class="bar-count">{{ filter.priorities.length }}</span></summary>
      <fieldset class="bar-panel" aria-label="Priority">
        <label v-for="priority in PRIORITY_ORDER" :key="priority" class="bar-check">
          <input
            type="checkbox"
            :checked="filter.priorities.includes(priority)"
            @change="setFilter({ ...filter, priorities: toggled(filter.priorities, priority) })"
          />
          <span class="priority" :data-priority="priority" aria-hidden="true"><i></i><i></i><i></i></span>
          {{ priorityLabel(priority) }}
        </label>
      </fieldset>
    </details>
    <details class="bar-menu" :data-active="filter.labels.length > 0">
      <summary>Label<span v-if="filter.labels.length > 0" class="bar-count">{{ filter.labels.length }}</span></summary>
      <fieldset class="bar-panel" aria-label="Label">
        <p v-if="labelChoices.length === 0" class="bar-panel-heading">No issue has a label.</p>
        <p v-if="labelsFull" id="label-cap" class="bar-panel-note">
          At most {{ LIMITS.labels }} labels at once: clear one to choose another.
        </p>
        <label v-for="label in labelChoices" :key="label" class="bar-check">
          <input
            type="checkbox"
            :checked="filter.labels.includes(label)"
            :disabled="labelsFull && !filter.labels.includes(label)"
            :aria-describedby="labelsFull ? 'label-cap' : undefined"
            @change="setFilter({ ...filter, labels: toggled(filter.labels, label) })"
          />
          {{ label }}
        </label>
      </fieldset>
    </details>
    <label class="bar-field">
      <span class="visually-hidden">Assignee</span>
      <select
        class="bar-select"
        :data-active="filter.assignee !== undefined"
        :value="assigneeValue(filter.assignee)"
        @change="assignee"
      >
        <option value="">Any assignee</option>
        <option value="nobody">Assigned to nobody</option>
        <option value="human">Assigned to you</option>
        <option v-for="role in assigneeRoles" :key="role" :value="`role:${role}`">Assigned to {{ role }}</option>
      </select>
    </label>
    <label class="bar-field">
      <span class="visually-hidden">Claim</span>
      <select
        class="bar-select"
        :data-active="filter.claim !== undefined"
        :value="filter.claim ?? ''"
        @change="claim"
      >
        <option value="">Claimed or not</option>
        <option value="mine">Claimed by you</option>
        <option value="any">Claimed by anyone</option>
        <option value="none">Claimed by nobody</option>
      </select>
    </label>
    <label class="bar-field">
      <span class="visually-hidden">Ready</span>
      <select
        class="bar-select"
        :data-active="filter.ready !== undefined"
        :value="filter.ready === undefined ? '' : String(filter.ready)"
        @change="ready"
      >
        <option value="">Ready or not</option>
        <option value="true">Ready</option>
        <option value="false">Not ready</option>
      </select>
    </label>
    <label class="bar-field bar-search">
      <span class="visually-hidden">Search</span>
      <input
        id="filter-text"
        class="bar-input"
        type="search"
        placeholder="Search…  /"
        autocomplete="off"
        :value="filter.text"
        @input="search"
        @keydown="leaveSearch"
      />
    </label>
    <button v-if="filtered" class="ghost-button" type="button" @click="setFilter(NO_FILTER)">Clear</button>
    <form v-if="saving.kind === 'naming'" class="bar-save" @submit="save" @keydown="cancelSaving">
      <label class="bar-field">
        <span class="visually-hidden">Name of the view</span>
        <input
          id="view-name"
          class="bar-input"
          placeholder="Name of the view"
          autocomplete="off"
          :value="saving.name"
          @input="nameInput"
        />
      </label>
      <p v-if="notSaved" class="field-error" role="alert">
        The view could not be saved: the server did not keep it. Try again.
      </p>
      <button
        class="primary-button"
        type="submit"
        :disabled="saveProblem !== undefined || saveState === 'pending'"
      >
        {{ replacing === undefined ? "Save" : `Replace ${replacing.name}` }}
      </button>
      <button class="ghost-button" type="button" @click="closeSaving">Cancel</button>
    </form>
    <button
      v-else-if="filtered"
      id="save-view"
      class="ghost-button"
      type="button"
      @click="startSaving"
    >Save view</button>
  </div>
</template>
