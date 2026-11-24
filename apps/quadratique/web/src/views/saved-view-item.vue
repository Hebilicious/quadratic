<script setup lang="ts" vapor>
import { computed, nextTick, ref } from "vue";

import { nameProblem, refusedWrite, type SavedView } from "../board/saved-views.machine.ts";
import { useSnapshot } from "../use-actor.ts";
import { useWorkspace } from "../workspace.ts";

const props = defineProps<{
  readonly view: SavedView;
  readonly views: readonly SavedView[];
  readonly href: string;
  readonly current: boolean;
}>();
const emit = defineEmits<{ follow: [event: MouseEvent] }>();
const { savedViews } = useWorkspace();
const savedSnapshot = useSnapshot(savedViews);

type Editing =
  | { readonly kind: "closed" }
  | { readonly kind: "renaming"; readonly name: string }
  | { readonly kind: "deleting" };
const editing = ref<Editing>({ kind: "closed" });
const renameForm = computed(() => `rename:${props.view.id}`);
const removeForm = computed(() => `remove:${props.view.id}`);
const problem = computed(() =>
  editing.value.kind === "renaming"
    ? nameProblem(props.views, editing.value.name, props.view.id)
    : undefined,
);

function forget(): void {
  savedViews.send({ type: "forget", form: renameForm.value });
  savedViews.send({ type: "forget", form: removeForm.value });
}

async function edit(): Promise<void> {
  editing.value = { kind: "renaming", name: props.view.name };
  await nextTick();
  const input = document.getElementById(`rename-${props.view.id}`);
  if (input instanceof HTMLInputElement) {
    input.focus();
    input.select();
  }
}

function rename(event: Event): void {
  event.preventDefault();
  if (editing.value.kind !== "renaming" || problem.value !== undefined) {
    return;
  }
  const form = renameForm.value;
  savedViews.send({ type: "rename", form, id: props.view.id, name: editing.value.name });
  if (!refusedWrite(savedViews.getSnapshot(), form)) {
    void close();
  }
}

async function close(): Promise<void> {
  forget();
  editing.value = { kind: "closed" };
  await nextTick();
  document.getElementById(`edit-${props.view.id}`)?.focus();
}

async function confirmDelete(): Promise<void> {
  forget();
  editing.value = { kind: "deleting" };
  await nextTick();
  document.getElementById(`keep-${props.view.id}`)?.focus();
}

function remove(): void {
  savedViews.send({ type: "remove", form: removeForm.value, id: props.view.id });
}

const notSaved = computed(() => {
  const current = editing.value;
  return (
    (current.kind === "renaming" && refusedWrite(savedSnapshot.value, renameForm.value)) ||
    (current.kind === "deleting" && refusedWrite(savedSnapshot.value, removeForm.value))
  );
});

function typed(event: Event): void {
  if (event.target instanceof HTMLInputElement) {
    savedViews.send({ type: "forget", form: renameForm.value });
    editing.value = { kind: "renaming", name: event.target.value };
  }
}

function cancel(event: KeyboardEvent): void {
  if (event.key === "Escape") {
    event.preventDefault();
    void close();
  }
}
</script>

<template>
  <div class="saved-view">
    <div v-if="editing.kind === 'closed'" class="saved-view-row">
      <a
        class="nav-link"
        data-named="true"
        :href="href"
        :aria-current="current ? 'page' : undefined"
        @click="emit('follow', $event)"
      >
        <span class="nav-key" aria-hidden="true">≡</span>
        <span class="nav-name">{{ view.name }}</span>
      </a>
      <button
        :id="`edit-${view.id}`"
        class="saved-view-edit ghost-button"
        type="button"
        :aria-label="`Rename or delete the view ${view.name}`"
        @click="edit"
      >…</button>
    </div>
    <form
      v-else-if="editing.kind === 'renaming'"
      class="saved-view-form"
      @submit="rename"
      @keydown="cancel"
    >
      <label>
        <span class="visually-hidden">New name of the view {{ view.name }}</span>
        <input
          :id="`rename-${view.id}`"
          class="bar-input"
          autocomplete="off"
          :value="editing.name"
          @input="typed"
        />
      </label>
      <p v-if="problem !== undefined" class="field-error" role="alert">{{ problem }}</p>
      <p v-else-if="notSaved" class="field-error" role="alert">
        The view could not be renamed: this browser does not keep site data here.
      </p>
      <div class="saved-view-actions">
        <button class="primary-button" type="submit" :disabled="problem !== undefined">Rename</button>
        <button class="ghost-button danger" type="button" @click="confirmDelete">Delete…</button>
        <button class="ghost-button" type="button" @click="close">Cancel</button>
      </div>
    </form>
    <div v-else class="saved-view-form" role="group" :aria-label="`Delete the view ${view.name}`">
      <p class="nav-note">Delete the view {{ view.name }}? Its issues stay as they are.</p>
      <p v-if="notSaved" class="field-error" role="alert">
        The view could not be deleted: this browser does not keep site data here.
      </p>
      <div class="saved-view-actions">
        <button class="primary-button" type="button" @click="remove">Delete</button>
        <button :id="`keep-${view.id}`" class="ghost-button" type="button" @click="close">Keep it</button>
      </div>
    </div>
  </div>
</template>
