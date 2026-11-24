<script setup lang="ts" vapor>
import type { Group } from "../board/view.ts";

defineProps<{ readonly group: Group; readonly heading: "group-header" | "column-header" }>();
</script>

<template>
  <h2 :class="heading">
    <span
      v-if="group.tone.kind === 'category'"
      class="status-icon"
      :data-category="group.tone.category"
      aria-hidden="true"
    ></span>
    <span
      v-else-if="group.tone.kind === 'priority'"
      class="priority"
      :data-priority="group.tone.priority"
      aria-hidden="true"
    ><i></i><i></i><i></i></span>
    <span class="group-name">{{ group.name }}</span>
    <span class="group-count">{{ group.issues.length }}</span>
  </h2>
</template>
