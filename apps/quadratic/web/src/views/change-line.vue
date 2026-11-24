<script setup lang="ts" vapor>
import type { ChangeLine } from "./present.ts";

defineProps<{ readonly line: ChangeLine }>();
</script>

<template>
  <li>
    <details v-if="line.long" class="change-long">
      <summary><span class="change-field">{{ line.label }}:</span> {{ line.before === undefined ? "set" : line.after === undefined ? "removed" : "changed" }}</summary>
      <p v-if="line.before !== undefined" class="change-value muted" :data-exact="line.exact">Before: {{ line.before }}</p>
      <p v-if="line.after !== undefined" class="change-value" :data-exact="line.exact">After: {{ line.after }}</p>
    </details>
    <template v-else><span class="change-field">{{ line.label }}:</span> <span v-if="line.before === undefined" :data-exact="line.exact">{{ line.after }}</span><span v-else-if="line.after === undefined"><span class="muted change-removed" :data-exact="line.exact">{{ line.before }}</span> <span class="muted">removed</span></span><span v-else><span class="muted" :data-exact="line.exact">{{ line.before }}</span> → <span :data-exact="line.exact">{{ line.after }}</span></span></template>
  </li>
</template>
