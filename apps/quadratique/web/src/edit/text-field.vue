<script setup lang="ts" vapor>
import { computed, nextTick, ref } from "vue";

import { failureText } from "../views/present.ts";
import type { IssuePatch } from "../core/quadratique.ts";
import { useWorkspace } from "../workspace.ts";
import type { IssueEditor } from "./issue-editor.ts";
import { useEdit } from "./use-edit.ts";

const props = defineProps<{
  readonly label: string;
  readonly value: string | undefined;
  readonly editor: IssueEditor;
  readonly save: (text: string) => IssuePatch;
  readonly multiline: boolean;
  readonly required: boolean;
  readonly empty: string;
}>();

const { drafts } = useWorkspace();
const key = `${props.editor.issue} ${props.label}`;
const draft = ref("");
const edit = useEdit({
  run: async (base, signal) =>
    await props.editor.update(props.save(draft.value.trim()), base, signal),
  revision: () => props.editor.revision(),
  refresh: () => {
    props.editor.refresh();
  },
  saved: () => {
    drafts.drop(key);
  },
});
const { actor, phase } = edit;
const kept = drafts.get(key);
if (kept !== undefined) {
  draft.value = kept.text;
  actor.send({ type: "resume", base: kept.base });
}
const open = computed(() => phase.value !== "idle");
const blank = computed(() => props.required && draft.value.trim().length === 0);
const fieldId = `field-${Math.random().toString(36).slice(2)}`;

async function start(): Promise<void> {
  draft.value = props.value ?? "";
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
  const target = event.target;
  if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) {
    draft.value = target.value;
    drafts.keep(key, { text: target.value, base: edit.base.value });
  }
}

function cancel(): void {
  drafts.drop(key);
  actor.send({ type: "cancel" });
}

function onKey(event: KeyboardEvent): void {
  if (event.key === "Escape") {
    event.stopPropagation();
    event.preventDefault();
    cancel();
  } else if (event.key === "Enter" && (event.metaKey || event.ctrlKey || !props.multiline)) {
    event.preventDefault();
    save();
  }
}
</script>

<template>
  <section class="field" :data-phase="phase">
    <div class="field-head">
      <h3 class="field-label">{{ label }}</h3>
      <button
        v-if="!open"
        class="ghost-button"
        type="button"
        :aria-label="`Edit ${label.toLowerCase()}`"
        @click="start"
      >
        Edit
      </button>
    </div>
    <template v-if="open">
      <textarea
        v-if="multiline"
        :id="fieldId"
        class="field-input"
        rows="6"
        :aria-label="label"
        :value="draft"
        :disabled="phase === 'saving'"
        @input="onInput"
        @keydown="onKey"
      ></textarea>
      <input
        v-else
        :id="fieldId"
        class="field-input"
        :aria-label="label"
        :value="draft"
        :disabled="phase === 'saving'"
        @input="onInput"
        @keydown="onKey"
      />
      <p v-if="phase === 'refused' || phase === 'stale'" class="field-error" role="alert">
        {{ failureText(edit.error.value) }}
      </p>
      <p v-if="phase === 'stale'" class="field-now">
        Now: {{ value ?? empty }}
      </p>
      <div class="field-actions">
        <button
          class="primary-button"
          type="button"
          :disabled="phase === 'saving' || (phase === 'stale' && !edit.refreshed.value) || blank"
          @click="save"
        >
          {{ phase === "saving" ? "Saving…" : phase === "stale" ? "Save again" : "Save" }}
        </button>
        <button class="ghost-button" type="button" :disabled="phase === 'saving'" @click="cancel">
          Cancel
        </button>
        <span class="field-hint"><kbd>{{ multiline ? "Ctrl ↵" : "↵" }}</kbd> to save, <kbd>Esc</kbd> to cancel</span>
      </div>
    </template>
    <p
      v-else-if="value !== undefined"
      class="field-value"
      :data-multiline="multiline"
      @dblclick="start"
    >{{ value }}</p>
    <p v-else class="field-value field-empty" @dblclick="start">{{ empty }}</p>
  </section>
</template>
