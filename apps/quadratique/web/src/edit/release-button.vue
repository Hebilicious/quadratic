<script setup lang="ts" vapor>
import { failureText } from "../views/present.ts";
import type { IssueEditor } from "./issue-editor.ts";
import { useEdit } from "./use-edit.ts";

const props = defineProps<{ readonly editor: IssueEditor; readonly holder: string }>();

const edit = useEdit({
  run: async (_, signal) => await props.editor.release(signal),
  revision: () => props.editor.revision(),
  refresh: () => {
    props.editor.refresh();
  },
});
const { actor, phase } = edit;
</script>

<template>
  <div class="release">
    <button
      class="ghost-button danger"
      type="button"
      :disabled="phase === 'saving'"
      @click="actor.send({ type: 'submit' })"
    >{{ phase === "saving" ? "Releasing…" : `Release ${holder}'s claim` }}</button>
    <p v-if="phase === 'refused' || phase === 'stale'" class="field-error" role="alert">
      {{ failureText(edit.error.value) }}
      <button class="link-button" type="button" @click="actor.send({ type: 'cancel' })">Dismiss</button>
    </p>
  </div>
</template>
