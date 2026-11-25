<script setup lang="ts" vapor>
import { computed } from "vue";

import { textParts } from "./present.ts";

const props = defineProps<{ readonly text: string }>();
const parts = computed(() => textParts(props.text));
</script>

<template>
  <template v-for="(part, index) in parts" :key="index">
    <a
      v-if="part.kind === 'link'"
      class="text-link"
      :href="part.text"
      target="_blank"
      rel="noopener noreferrer"
    >{{ part.text }}</a>
    <template v-else>{{ part.text }}</template>
  </template>
</template>
