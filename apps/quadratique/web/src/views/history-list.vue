<script setup lang="ts" vapor>
import { computed } from "vue";

import type { DetailActor } from "../board/detail.machine.ts";
import type { HistoryChange } from "../board/model.ts";
import { useSnapshot } from "../use-actor.ts";
import { ago, changeLines, operationText, when } from "./present.ts";
import ChangeLineItem from "./change-line.vue";

const props = defineProps<{
  readonly detail: DetailActor;
  readonly statusName: (key: string) => string;
}>();
const snapshot = useSnapshot(props.detail);
const state = computed(() => {
  const value = snapshot.value;
  if (value.matches({ history: "hidden" })) {
    return "hidden";
  }
  if (value.matches({ history: "failed" })) {
    return "failed";
  }
  return value.matches({ history: "loading" }) ? "loading" : "shown";
});
const loadingMore = computed(() => snapshot.value.matches({ history: { shown: "more" } }));
const changes = computed((): readonly HistoryChange[] => snapshot.value.context.history);
const more = computed((): boolean => snapshot.value.context.historyCursor !== undefined);
</script>

<template>
  <section class="history" aria-label="Changes">
    <div class="field-head">
      <h3 class="field-label">Changes</h3>
      <button
        v-if="state === 'hidden' || state === 'failed'"
        class="ghost-button"
        type="button"
        @click="detail.send({ type: 'showHistory' })"
      >Show the changes</button>
    </div>
    <p v-if="state === 'failed'" class="field-error" role="alert">The changes did not load.</p>
    <p v-else-if="state === 'loading'" class="field-empty" aria-busy="true">Loading the changes…</p>
    <ol v-else-if="state === 'shown'" class="changes">
      <li v-for="change in changes" :key="`${change.commit}-${change.issue ?? ''}`" class="change">
        <p class="change-head">
          <span class="change-author">{{ change.author.name }}</span>
          <span class="change-operation" :title="change.operation">{{ operationText(change.operation) }}</span>
          <span v-if="change.branch !== undefined" class="muted">on <code>{{ change.branch }}</code></span>
          <span class="muted change-when" :title="when(change.at)">{{ ago(change.at) }}</span>
        </p>
        <p v-if="change.reason !== undefined" class="change-reason">{{ change.reason }}</p>
        <ul class="change-fields">
          <ChangeLineItem
            v-for="line in changeLines(change.fields, statusName)"
            :key="`${line.field}-${line.before ?? ''}-${line.after ?? ''}`"
            :line="line"
          />
        </ul>
      </li>
      <li v-if="changes.length === 0" class="field-empty">No change yet.</li>
    </ol>
    <button
      v-if="state === 'shown' && more"
      class="ghost-button"
      type="button"
      :disabled="loadingMore"
      @click="detail.send({ type: 'moreHistory' })"
    >{{ loadingMore ? "Loading…" : "Show older changes" }}</button>
  </section>
</template>
