<script setup lang="ts" vapor>
import { computed, onMounted, onScopeDispose } from "vue";
import { createActor } from "xstate";

import { rolesOf } from "../board/issues.ts";
import type { Project } from "../board/model.ts";
import { canSubmit, createMachine, newIssue, type DraftField } from "../edit/create.machine.ts";
import { useSnapshot } from "../use-actor.ts";
import { useWorkspace } from "../workspace.ts";
import { useModal } from "./modal.ts";
import { failureText, labelsOf, priorityLabel, PRIORITY_CHOICES } from "./present.ts";

const props = defineProps<{ readonly project: Project | undefined }>();
const { board, keys, quadratique } = useWorkspace();
const boardSnapshot = useSnapshot(board);
const projects = computed(() => boardSnapshot.value.context.projects);

const form = createActor(
  createMachine({
    create: async (draft, signal) =>
      await quadratique.create(newIssue(draft, labelsOf(draft.labels)), signal),
    parents: async (project, signal) => await quadratique.matching({ project }, signal),
    created: (issue) => {
      keys.send({ type: "created", issue: issue.id });
      keys.send({ type: "close" });
    },
    dismissed: () => {
      keys.send({ type: "close" });
    },
  }),
  { input: { project: props.project?.key ?? projects.value[0]?.key ?? "" } },
);
form.start();
onScopeDispose(() => {
  form.stop();
});
const snapshot = useSnapshot(form);
const draft = computed(() => snapshot.value.context.draft);
const submitting = computed(() => !snapshot.value.matches({ form: "editing" }));
const chosen = computed(() => projects.value.find((each) => each.key === draft.value.project));
const roles = computed(() =>
  rolesOf(boardSnapshot.value.context.issues, boardSnapshot.value.context.now),
);
const parents = computed(() =>
  snapshot.value.context.parents.toSorted((left, right) =>
    left.id.localeCompare(right.id, undefined, { numeric: true }),
  ),
);
const error = computed(() => snapshot.value.context.error);

function value(event: Event): string {
  const target = event.target;
  return target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLSelectElement
    ? target.value
    : "";
}

function set(name: DraftField, event: Event): void {
  form.send({ type: "field", name, value: value(event) });
}

function dismiss(): void {
  form.send({ type: "dismiss" });
}

function onKey(event: KeyboardEvent): void {
  if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    dismiss();
  } else if (
    event.key === "Enter" &&
    (event.metaKey || event.ctrlKey || event.target instanceof HTMLInputElement)
  ) {
    event.preventDefault();
    form.send({ type: "submit" });
  }
}

onMounted(() => {
  document.getElementById("create-title")?.focus();
});

useModal("create-dialog");
</script>

<template>
  <div class="backdrop" @click.self="dismiss">
    <div
      id="create-dialog"
      class="dialog create"
      role="dialog"
      aria-modal="true"
      aria-labelledby="create-heading"
      :aria-busy="submitting"
      @keydown="onKey"
    >
      <div class="dialog-head">
        <h2 id="create-heading" class="dialog-title">New issue</h2>
        <label class="inline-choice">
          <span class="muted">in</span>
          <select
            class="choice-select"
            aria-label="Project"
            :value="draft.project"
            :disabled="submitting"
            @change="form.send({ type: 'project', key: value($event) })"
          >
            <option
              v-for="each in projects"
              :key="each.key"
              :value="each.key"
              :selected="each.key === draft.project"
            >{{ each.key }} {{ each.name }}</option>
          </select>
        </label>
      </div>
      <input
        id="create-title"
        class="create-title"
        aria-label="Title"
        placeholder="Title"
        maxlength="200"
        :value="draft.title"
        :readonly="submitting"
        @input="set('title', $event)"
      />
      <textarea
        class="field-input"
        rows="4"
        aria-label="Description"
        placeholder="Description (optional)"
        :value="draft.description"
        :readonly="submitting"
        @input="set('description', $event)"
      ></textarea>
      <textarea
        class="field-input"
        rows="3"
        aria-label="Acceptance criteria"
        placeholder="Acceptance criteria: when is it done?"
        required
        :value="draft.acceptance"
        :readonly="submitting"
        @input="set('acceptance', $event)"
      ></textarea>
      <div class="create-fields">
        <label class="choice-label">
          <span>Status</span>
          <select
            class="choice-select"
            :value="draft.status"
            :disabled="submitting"
            @change="set('status', $event)"
          >
            <option value="" :selected="draft.status === ''">Default</option>
            <option
              v-for="each in chosen?.workflow.statuses ?? []"
              :key="each.key"
              :value="each.key"
              :selected="each.key === draft.status"
            >{{ each.name }}</option>
          </select>
        </label>
        <label class="choice-label">
          <span>Priority</span>
          <select
            class="choice-select"
            :value="draft.priority"
            :disabled="submitting"
            @change="set('priority', $event)"
          >
            <option
              v-for="each in PRIORITY_CHOICES"
              :key="each"
              :value="String(each)"
              :selected="String(each) === draft.priority"
            >{{ priorityLabel(each) }}</option>
          </select>
        </label>
        <label class="choice-label">
          <span>Assignee</span>
          <select
            class="choice-select"
            :value="draft.assignee"
            :disabled="submitting"
            @change="set('assignee', $event)"
          >
            <option value="" :selected="draft.assignee === ''">Nobody</option>
            <option value="human" :selected="draft.assignee === 'human'">You</option>
            <option
              v-for="role in roles"
              :key="role"
              :value="`role:${role}`"
              :selected="draft.assignee === `role:${role}`"
            >Role {{ role }}</option>
          </select>
        </label>
        <label class="choice-label">
          <span>Parent</span>
          <select
            class="choice-select"
            :value="draft.parent"
            :disabled="submitting"
            @change="set('parent', $event)"
          >
            <option value="" :selected="draft.parent === ''">None</option>
            <option
              v-for="each in parents"
              :key="each.id"
              :value="each.id"
              :selected="each.id === draft.parent"
            >{{ each.id }} {{ each.title }}</option>
          </select>
        </label>
        <label class="choice-label">
          <span>Labels</span>
          <input
            class="field-input"
            placeholder="web, ui"
            :value="draft.labels"
            :readonly="submitting"
            @input="set('labels', $event)"
          />
        </label>
      </div>
      <p v-if="error !== undefined && !submitting" class="field-error" role="alert">
        {{ failureText(error) }}
      </p>
      <div class="field-actions">
        <button
          class="primary-button"
          type="button"
          :disabled="!canSubmit(draft) || submitting"
          @click="form.send({ type: 'submit' })"
        >{{ submitting ? "Creating…" : "Create issue" }}</button>
        <button class="ghost-button" type="button" :disabled="submitting" @click="dismiss">Cancel</button>
        <span class="field-hint"><kbd>↵</kbd> in the title or <kbd>Ctrl ↵</kbd> to create</span>
      </div>
    </div>
  </div>
</template>
