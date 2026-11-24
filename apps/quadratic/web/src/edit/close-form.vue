<script setup lang="ts" vapor>
import { nextTick, onMounted, ref } from "vue";

import { failureText } from "../views/present.ts";
import type { IssueEditor } from "./issue-editor.ts";
import { useEdit } from "./use-edit.ts";

const props = defineProps<{ readonly editor: IssueEditor; readonly focus: boolean }>();

const draft = ref("");
const edit = useEdit({
  run: async (_, signal) => await props.editor.close(draft.value.trim(), signal),
  revision: () => props.editor.revision(),
  refresh: () => {
    props.editor.refresh();
  },
});
const { actor, phase } = edit;
const fieldId = `close-${Math.random().toString(36).slice(2)}`;

async function open(): Promise<void> {
  actor.send({ type: "start" });
  await nextTick();
  document.getElementById(fieldId)?.focus();
}

function cancel(): void {
  draft.value = "";
  actor.send({ type: "cancel" });
}

onMounted(async () => {
  if (props.focus) {
    await open();
  }
});

function onInput(event: Event): void {
  if (event.target instanceof HTMLTextAreaElement) {
    draft.value = event.target.value;
    if (phase.value === "refused") {
      actor.send({ type: "start" });
    }
  }
}

function onKey(event: KeyboardEvent): void {
  if (event.key === "Escape") {
    event.stopPropagation();
    event.preventDefault();
    cancel();
  } else if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
    event.preventDefault();
    close();
  }
}

function close(): void {
  if (draft.value.trim().length === 0) {
    return;
  }
  actor.send({ type: "save" });
}
</script>

<template>
  <section class="field close-form" aria-label="Close with evidence">
    <div class="field-head">
      <h3 class="field-label">Close with evidence</h3>
      <button v-if="phase === 'idle'" class="ghost-button" type="button" @click="open">Close…</button>
    </div>
    <textarea
      v-if="phase !== 'idle'"
      :id="fieldId"
      class="field-input"
      rows="3"
      aria-label="Evidence"
      placeholder="What was checked, and how: commands, results, links."
      :value="draft"
      :disabled="phase === 'saving'"
      @input="onInput"
      @keydown="onKey"
    ></textarea>
    <p v-if="phase === 'refused' || phase === 'stale'" class="field-error" role="alert">
      {{ failureText(edit.error.value) }}
    </p>
    <div v-if="phase !== 'idle'" class="field-actions">
      <button
        class="primary-button"
        type="button"
        :disabled="phase === 'saving' || draft.trim().length === 0"
        @click="close"
      >{{ phase === "saving" ? "Closing…" : "Close the issue" }}</button>
      <button class="ghost-button" type="button" :disabled="phase === 'saving'" @click="cancel">Cancel</button>
      <span class="field-hint">It moves to the merge status of the workflow, and the claim ends.</span>
    </div>
  </section>
</template>
