<script setup lang="ts" vapor>
import { computed } from "vue";

import { useSnapshot } from "../use-actor.ts";
import { useWorkspace } from "../workspace.ts";
import { failureText } from "./present.ts";

const { commands } = useWorkspace();
const snapshot = useSnapshot(commands);
const running = computed(() => snapshot.value.matches("running"));
const failed = computed(() =>
  snapshot.value.matches("reported") ? snapshot.value.context.failed : [],
);
</script>

<template>
  <div class="toasts" aria-live="polite">
    <p v-if="running" class="toast">Saving…</p>
    <div v-if="failed.length > 0" class="toast toast-error" role="alert">
      <ul class="toast-list">
        <li v-for="(each, index) in failed" :key="`${each.issue}-${index}`">
          <strong v-if="each.issue !== ''">{{ each.issue }}:</strong> {{ failureText(each.error) }}
        </li>
      </ul>
      <button class="ghost-button" type="button" @click="commands.send({ type: 'dismiss' })">Dismiss</button>
    </div>
  </div>
</template>
