<script setup lang="ts" vapor>
import { computed, ref } from "vue";

import type { Issue, IssueSummary, RelationKind } from "../board/model.ts";
import { paths } from "../route.ts";
import { useWorkspace } from "../workspace.ts";
import { failureText, RELATION_KINDS, relationKindLabel, relationLabel } from "../views/present.ts";
import type { IssueEditor, LinkChange } from "./issue-editor.ts";
import { useEdit } from "./use-edit.ts";

const props = defineProps<{
  readonly issue: Issue;
  readonly others: readonly IssueSummary[];
  readonly editor: IssueEditor;
  readonly statusName: (status: string) => string;
}>();
const { router } = useWorkspace();

const change = ref<LinkChange | undefined>(undefined);
const edit = useEdit({
  run: async (_, signal) => {
    if (change.value !== undefined) {
      await props.editor.link(change.value, signal);
    }
  },
  revision: () => props.editor.revision(),
  refresh: () => {
    props.editor.refresh();
  },
});
const { actor, phase } = edit;

const kind = ref<RelationKind>("blocks");
const target = ref("");
const sorted = computed(() =>
  props.issue.relations.toSorted(
    (left, right) =>
      RELATION_KINDS.indexOf(left.kind) - RELATION_KINDS.indexOf(right.kind) ||
      left.direction.localeCompare(right.direction),
  ),
);

function onKind(event: Event): void {
  if (event.target instanceof HTMLSelectElement) {
    const chosen = RELATION_KINDS.find(
      (each) => each === (event.target as HTMLSelectElement).value,
    );
    if (chosen !== undefined) {
      kind.value = chosen;
    }
  }
}

function onTarget(event: Event): void {
  if (event.target instanceof HTMLSelectElement) {
    target.value = event.target.value;
  }
}

function add(): void {
  if (target.value !== "") {
    change.value = { from: props.issue.id, kind: kind.value, target: target.value, remove: false };
    actor.send({ type: "submit" });
    target.value = "";
  }
}

function remove(relation: Issue["relations"][number]): void {
  change.value =
    relation.direction === "outgoing"
      ? { from: props.issue.id, kind: relation.kind, target: relation.issue, remove: true }
      : { from: relation.issue, kind: relation.kind, target: props.issue.id, remove: true };
  actor.send({ type: "submit" });
}

function follow(event: MouseEvent): void {
  const link = event.currentTarget;
  if (
    !(link instanceof HTMLAnchorElement) ||
    event.button !== 0 ||
    event.metaKey ||
    event.ctrlKey
  ) {
    return;
  }
  event.preventDefault();
  router.go(link.pathname);
}
</script>

<template>
  <section class="side-section" aria-label="Relations">
    <h3 class="field-label">Relations</h3>
    <ul v-if="sorted.length > 0" class="links">
      <li
        v-for="relation in sorted"
        :key="`${relation.kind}-${relation.direction}-${relation.issue}`"
        class="relation"
      >
        <span class="muted">{{ relationLabel(relation) }}</span>
        <a class="issue-link" :href="paths.issue(relation.issue)" @click="follow">{{ relation.issue }}</a>
        <span class="relation-title">{{ relation.title }}</span>
        <span
          class="status-icon"
          :data-category="relation.category"
          :title="statusName(relation.status)"
          aria-hidden="true"
        ></span>
        <span class="muted">{{ statusName(relation.status) }}</span>
        <button
          class="ghost-button"
          type="button"
          :aria-label="`Remove: ${relationLabel(relation)} ${relation.issue}`"
          :disabled="phase === 'saving'"
          @click="remove(relation)"
        >Remove</button>
      </li>
    </ul>
    <p v-else class="field-empty">No relations.</p>
    <div class="relation-add">
      <select class="choice-select" aria-label="Relation" :value="kind" @change="onKind">
        <option v-for="each in RELATION_KINDS" :key="each" :value="each" :selected="each === kind">{{ relationKindLabel(each) }}</option>
      </select>
      <select class="choice-select" aria-label="Related issue" :value="target" @change="onTarget">
        <option value="" :selected="target === ''">Choose an issue…</option>
        <option
          v-for="other in others"
          :key="other.id"
          :value="other.id"
          :selected="other.id === target"
        >{{ other.id }} {{ other.title }}</option>
      </select>
      <button
        class="ghost-button"
        type="button"
        :disabled="target === '' || phase === 'saving'"
        @click="add"
      >Add</button>
    </div>
    <p v-if="phase === 'refused' || phase === 'stale'" class="field-error" role="alert">
      {{ failureText(edit.error.value) }}
      <button class="link-button" type="button" @click="actor.send({ type: 'cancel' })">Dismiss</button>
    </p>
  </section>
</template>
