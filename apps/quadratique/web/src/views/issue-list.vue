<script setup lang="ts" vapor>
import { computed } from "vue";

import { cursorGroups, type Group } from "../board/view.ts";
import GroupHeader from "./group-header.vue";
import IssueRow from "./issue-row.vue";

const props = defineProps<{
  readonly groups: readonly Group[];
  readonly state: "loading" | "ready" | "failed";
  readonly failure: string;
  readonly empty: string;
  readonly cursor: string | undefined;
  readonly selected: readonly string[];
}>();
const emit = defineEmits<{ retry: [] }>();
const stops = computed(() => cursorGroups(props.groups));
</script>

<template>
  <p v-if="state === 'failed'" class="empty">
    {{ failure }} <button class="link-button" type="button" @click="emit('retry')">Retry</button>
  </p>
  <p v-else-if="state === 'ready' && groups.length === 0" class="empty">{{ empty }}</p>
  <div v-else class="list" :aria-busy="state === 'loading'">
    <section
      v-for="group in groups"
      :key="group.key"
      class="group"
      :data-category="group.tone.kind === 'category' ? group.tone.category : undefined"
      :aria-label="group.name"
    >
      <GroupHeader :group="group" heading="group-header" />
      <ul class="rows" :aria-label="group.name">
        <IssueRow
          v-for="issue in group.issues"
          :key="issue.id"
          :issue="issue"
          :current="issue.id === cursor && stops.get(issue.id) === group.key"
          :selected="selected.includes(issue.id)"
        />
      </ul>
    </section>
  </div>
</template>
