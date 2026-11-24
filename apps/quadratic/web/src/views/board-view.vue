<script setup lang="ts" vapor>
import { computed, onScopeDispose } from "vue";
import { createActor } from "xstate";

import { dragMachine } from "../board/drag.machine.ts";
import { cursorGroups, type Group } from "../board/view.ts";
import { useSnapshot } from "../use-actor.ts";
import GroupHeader from "./group-header.vue";
import IssueCard from "./issue-card.vue";

const props = defineProps<{
  readonly groups: readonly Group[];
  readonly state: "loading" | "ready" | "failed";
  readonly failure: string;
  readonly draggable: boolean;
  readonly cursor: string | undefined;
  readonly selected: readonly string[];
}>();
const emit = defineEmits<{ retry: []; move: [move: { issue: string; group: string }] }>();

const drag = createActor(
  dragMachine((move) => {
    emit("move", move);
  }),
);
drag.start();
onScopeDispose(() => {
  drag.stop();
});
const dragSnapshot = useSnapshot(drag);
const stops = computed(() => cursorGroups(props.groups));
const dragging = computed(() => dragSnapshot.value.context.issue);
const over = computed(() => dragSnapshot.value.context.over);

function onStart(event: DragEvent, issue: string): void {
  if (!props.draggable) {
    event.preventDefault();
    return;
  }
  event.dataTransfer?.setData("text/plain", issue);
  if (event.dataTransfer !== null) {
    event.dataTransfer.effectAllowed = "move";
  }
  drag.send({ type: "start", issue });
}

function onOver(event: DragEvent, group: Group): void {
  if (dragging.value === undefined || group.move === undefined) {
    return;
  }
  event.preventDefault();
  if (over.value !== group.key) {
    drag.send({ type: "over", group: group.key });
  }
}

function onDrop(event: DragEvent, group: Group): void {
  event.preventDefault();
  drag.send({ type: "drop", group: group.key });
}
</script>

<template>
  <p v-if="state === 'failed'" class="empty">
    {{ failure }} <button class="link-button" type="button" @click="emit('retry')">Retry</button>
  </p>
  <div v-else class="board" :aria-busy="state === 'loading'">
    <section
      v-for="group in groups"
      :key="group.key"
      class="column"
      :data-category="group.tone.kind === 'category' ? group.tone.category : undefined"
      :data-over="over === group.key && dragging !== undefined"
      :aria-label="group.name"
      @dragover="onOver($event, group)"
      @drop="onDrop($event, group)"
    >
      <GroupHeader :group="group" heading="column-header" />
      <ul class="cards" :aria-label="group.name">
        <IssueCard
          v-for="issue in group.issues"
          :key="issue.id"
          :issue="issue"
          :current="issue.id === cursor && stops.get(issue.id) === group.key"
          :selected="selected.includes(issue.id)"
          :dragging="issue.id === dragging"
          :draggable="draggable"
          @dragstart="onStart($event, issue.id)"
          @dragend="drag.send({ type: 'end' })"
        />
      </ul>
    </section>
  </div>
</template>
