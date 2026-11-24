<script setup lang="ts" vapor>
import type { Head } from "../core/quadratique.ts";
import { when } from "./present.ts";

defineProps<{
  readonly state: "loading" | "ready" | "failed";
  readonly moment: Head | undefined;
  readonly asked: string;
  readonly nowHref: string;
  readonly historyHref: string;
}>();
const emit = defineEmits<{ follow: [event: MouseEvent] }>();
</script>

<template>
  <div class="moment-banner" role="note" aria-label="The past">
    <p class="moment-text">
      <strong>The past.</strong>
      <template v-if="state === 'loading'">Reading the board as it was…</template>
      <template v-else-if="state === 'failed'">There is no board at {{ asked }}.</template>
      <template v-else-if="moment === undefined">The board as it was before its first change: empty.</template>
      <template v-else>
        The board as it was at <time :datetime="moment.at">{{ when(moment.at) }}</time>, after the commit <code>{{ moment.commit.slice(0, 7) }}</code>. Nothing can be changed here.
      </template>
    </p>
    <a class="moment-link" :href="historyHref" @click="emit('follow', $event)">History</a>
    <a class="moment-now" :href="nowHref" @click="emit('follow', $event)">Back to now</a>
  </div>
</template>
