<script setup lang="ts" vapor>
import { computed, nextTick, ref } from "vue";

import type { Note } from "../board/model.ts";
import { ago, failureText, when } from "../views/present.ts";
import { useWorkspace } from "../workspace.ts";
import type { IssueEditor } from "./issue-editor.ts";
import { useEdit } from "./use-edit.ts";

const props = defineProps<{
  readonly notes: readonly Note[];
  readonly editor: IssueEditor;
}>();

const { drafts } = useWorkspace();
const key = `${props.editor.issue} Notes`;
const draft = ref("");
const edit = useEdit({
  run: async (base, signal) =>
    await props.editor.update({ notes: draft.value.trim() }, base, signal),
  revision: () => props.editor.revision(),
  refresh: () => {
    props.editor.refresh();
  },
  saved: () => {
    draft.value = "";
    drafts.drop(key);
  },
  landed: () => {
    const text = draft.value.trim();
    return text.length > 0 && props.notes.at(-1)?.text === text;
  },
});
const { actor, phase } = edit;
const kept = drafts.get(key);
if (kept !== undefined) {
  draft.value = kept.text;
  actor.send({ type: "resume", base: kept.base });
}
const open = computed(() => phase.value !== "idle");
const blank = computed(() => draft.value.trim().length === 0);
const fieldId = `note-${Math.random().toString(36).slice(2)}`;

async function start(): Promise<void> {
  actor.send({ type: "start" });
  await nextTick();
  document.getElementById(fieldId)?.focus();
}

function save(): void {
  if (!blank.value) {
    actor.send({ type: "save" });
  }
}

function onInput(event: Event): void {
  if (event.target instanceof HTMLTextAreaElement) {
    draft.value = event.target.value;
    drafts.keep(key, { text: event.target.value, base: edit.base.value });
  }
}

function cancel(): void {
  draft.value = "";
  drafts.drop(key);
  actor.send({ type: "cancel" });
}

function onKey(event: KeyboardEvent): void {
  if (event.key === "Escape") {
    event.stopPropagation();
    event.preventDefault();
    cancel();
  } else if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
    event.preventDefault();
    save();
  }
}
</script>

<template>
  <section class="field" aria-label="Notes" :data-phase="phase">
    <div class="field-head">
      <h3 class="field-label">Notes</h3>
      <button v-if="!open" class="ghost-button" type="button" @click="start">Add a note</button>
    </div>
    <ol v-if="notes.length > 0" class="notes">
      <li v-for="(note, index) in notes" :key="index" class="note">
        <p class="note-head">
          <span class="note-author">{{ note.author }}</span>
          <span class="muted" :title="when(note.at)">{{ ago(note.at) }}</span>
        </p>
        <p class="field-value" data-multiline="true">{{ note.text }}</p>
      </li>
    </ol>
    <p v-else class="field-value field-empty">No notes.</p>
    <template v-if="open">
      <textarea
        :id="fieldId"
        class="field-input"
        rows="3"
        aria-label="New note"
        :value="draft"
        :disabled="phase === 'saving'"
        @input="onInput"
        @keydown="onKey"
      ></textarea>
      <p v-if="phase === 'refused' || phase === 'stale'" class="field-error" role="alert">
        {{ failureText(edit.error.value) }}
      </p>
      <div class="field-actions">
        <button
          class="primary-button"
          type="button"
          :disabled="phase === 'saving' || (phase === 'stale' && !edit.refreshed.value) || blank"
          @click="save"
        >
          {{ phase === "saving" ? "Adding…" : phase === "stale" ? "Add again" : "Add the note" }}
        </button>
        <button class="ghost-button" type="button" :disabled="phase === 'saving'" @click="cancel">
          Cancel
        </button>
        <span class="field-hint"><kbd>Ctrl ↵</kbd> to add, <kbd>Esc</kbd> to cancel. A note cannot be changed or removed.</span>
      </div>
    </template>
  </section>
</template>
