<script setup lang="ts" vapor>
import { computed, ref } from "vue";

import { changeKey } from "../board/history.ts";
import type { HistoryChange, Project } from "../board/model.ts";
import { NO_FILTER, type Scope } from "../board/view.ts";
import { paths } from "../route.ts";
import { useSnapshot } from "../use-actor.ts";
import { useWorkspace } from "../workspace.ts";
import { ago, changeLines, operationText, when } from "./present.ts";
import ChangeLineItem from "./change-line.vue";

const props = defineProps<{ readonly scope: Scope; readonly projects: readonly Project[] }>();
const { board, router } = useWorkspace();
const snapshot = useSnapshot(board);

const state = computed(() => {
  const value = snapshot.value;
  if (value.matches({ history: "failed" })) {
    return "failed";
  }
  return value.matches({ history: "shown" }) ? "shown" : "loading";
});
const loadingMore = computed(() =>
  snapshot.value.matches({ history: { shown: { older: "loading" } } }),
);
const moreFailed = computed(() =>
  snapshot.value.matches({ history: { shown: { older: "failed" } } }),
);
const changes = computed((): readonly HistoryChange[] => snapshot.value.context.changes);
const more = computed(() => snapshot.value.context.changesCursor !== undefined);

const statusName = (project: string) => (key: string) =>
  props.projects
    .find((each) => each.key === project)
    ?.workflow.statuses.find((status) => status.key === key)?.name ?? key;

const thenPath = (moment: string) =>
  paths.board(props.scope, { group: "status", filter: NO_FILTER, at: moment });

function follow(event: MouseEvent): void {
  const link = event.currentTarget;
  if (
    !(link instanceof HTMLAnchorElement) ||
    event.button !== 0 ||
    event.metaKey ||
    event.ctrlKey ||
    event.shiftKey ||
    event.altKey
  ) {
    return;
  }
  event.preventDefault();
  router.go(link.pathname + link.search + link.hash);
}

function utcOf(local: string): string | undefined {
  const time = new Date(local).getTime();
  return Number.isNaN(time) ? undefined : `${new Date(time).toISOString().slice(0, 19)}Z`;
}

const typed = ref("");
const typedMoment = computed(() => utcOf(typed.value));

function onTyped(event: Event): void {
  if (event.target instanceof HTMLInputElement) {
    typed.value = event.target.value;
  }
}

function showThen(event: Event): void {
  event.preventDefault();
  if (typedMoment.value !== undefined) {
    router.go(thenPath(typedMoment.value));
  }
}
</script>

<template>
  <div class="history-view">
    <form class="moment-form" aria-label="The board at a moment" @submit="showThen">
      <label class="bar-field">
        <span class="bar-label">The board as it was at</span>
        <input
          class="bar-input"
          type="datetime-local"
          step="1"
          :value="typed"
          aria-label="Date and time"
          @input="onTyped"
        />
      </label>
      <button class="ghost-button" type="submit" :disabled="typedMoment === undefined">Show that board</button>
    </form>
    <p v-if="state === 'failed'" class="empty">
      The changes did not load.
      <button class="link-button" type="button" @click="board.send({ type: 'retry' })">Retry</button>
    </p>
    <p v-else-if="state === 'loading'" class="empty" aria-busy="true">Loading the changes…</p>
    <template v-else>
      <p v-if="changes.length === 0" class="empty">No change yet.</p>
      <ol class="board-changes" aria-label="Changes, newest first">
        <li v-for="change in changes" :key="changeKey(change)" class="board-change">
          <p class="change-head">
            <span class="change-what"><span class="change-author">{{ change.author.name }}</span> <span class="change-operation" :title="change.operation">{{ operationText(change.operation) }}</span> <a
                v-if="change.issue !== undefined"
                class="issue-id"
                :href="paths.issue(change.issue)"
                @click="follow"
              >{{ change.issue }}</a><span v-else class="muted">{{ change.project }}</span><span v-if="change.branch !== undefined" class="muted"> on <code>{{ change.branch }}</code></span></span>
            <span class="muted change-when" :title="when(change.at)">{{ ago(change.at) }}</span>
            <a
              class="change-then"
              :href="thenPath(change.commit)"
              :aria-label="`The board after this change, at ${when(change.at)}`"
              @click="follow"
            >Board then</a>
          </p>
          <p v-if="change.reason !== undefined" class="change-reason">{{ change.reason }}</p>
          <ul class="change-fields">
            <ChangeLineItem
              v-for="line in changeLines(change.fields, statusName(change.project))"
              :key="`${line.field}-${line.before ?? ''}-${line.after ?? ''}`"
              :line="line"
            />
          </ul>
        </li>
      </ol>
      <p v-if="moreFailed" class="empty" role="alert">
        The older changes did not load.
        <button class="link-button" type="button" @click="board.send({ type: 'moreChanges' })">Retry</button>
      </p>
      <button
        v-else-if="more"
        class="ghost-button history-more"
        type="button"
        :disabled="loadingMore"
        @click="board.send({ type: 'moreChanges' })"
      >{{ loadingMore ? "Loading…" : "Show older changes" }}</button>
    </template>
  </div>
</template>
