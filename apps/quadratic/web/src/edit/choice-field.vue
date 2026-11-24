<script setup lang="ts" vapor>
import { computed, ref } from "vue";

import type { IssuePatch } from "../core/quadratic.ts";
import { failureText } from "../views/present.ts";
import type { IssueEditor } from "./issue-editor.ts";
import { useEdit } from "./use-edit.ts";

const props = defineProps<{
  readonly label: string;
  readonly value: string;
  readonly options: readonly { readonly value: string; readonly label: string }[];
  readonly editor: IssueEditor;
  readonly save: (value: string) => IssuePatch;
}>();

const chosen = ref("");
const edit = useEdit({
  run: async (base, signal) => await props.editor.update(props.save(chosen.value), base, signal),
  revision: () => props.editor.revision(),
  refresh: () => {
    props.editor.refresh();
  },
});
const { actor, phase, refreshed } = edit;

function onChange(event: Event): void {
  const target = event.target;
  if (target instanceof HTMLSelectElement && target.value !== props.value) {
    chosen.value = target.value;
    actor.send({ type: "submit" });
  }
}

const shown = computed(() => (phase.value === "saving" ? chosen.value : props.value));

const labelOf = (value: string | undefined) =>
  props.options.find((option) => option.value === value)?.label ?? value ?? "";
</script>

<template>
  <div class="choice" :data-phase="phase">
    <label class="choice-label">
      <span>{{ label }}</span>
      <select class="choice-select" :value="shown" :disabled="phase === 'saving'" @change="onChange">
        <option
          v-for="option in options"
          :key="option.value"
          :value="option.value"
          :selected="option.value === shown"
        >{{ option.label }}</option>
      </select>
    </label>
    <div v-if="phase === 'refused' || phase === 'stale'" class="choice-problem" role="alert">
      <p class="field-error">{{ failureText(edit.error.value) }}</p>
      <p class="field-now">
        You chose {{ labelOf(chosen) }};
        {{
          phase === "stale" && !refreshed
            ? "reading the issue as it is now…"
            : `it is now ${labelOf(value)}.`
        }}
      </p>
      <div class="field-actions">
        <button
          class="primary-button"
          type="button"
          :disabled="phase === 'stale' && !refreshed"
          @click="actor.send({ type: 'save' })"
        >
          {{ phase === "stale" ? `Set ${labelOf(chosen)} again` : "Try again" }}
        </button>
        <button class="ghost-button" type="button" @click="actor.send({ type: 'cancel' })">Dismiss</button>
      </div>
    </div>
  </div>
</template>
