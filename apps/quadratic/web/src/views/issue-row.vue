<script setup lang="ts" vapor>
import type { IssueSummary } from "../board/model.ts";
import { useIssuePointer, useStatusName } from "./pointer.ts";
import IssueMeta from "./issue-meta.vue";
import { priorityLabel } from "./present.ts";

const props = defineProps<{
  readonly issue: IssueSummary;
  readonly current: boolean;
  readonly selected: boolean;
}>();
const pointer = useIssuePointer(() => props.issue.id);
const status = useStatusName();
</script>

<template>
  <li
    class="row"
    :data-issue="issue.id"
    :tabindex="current ? 0 : -1"
    :aria-current="current ? 'true' : undefined"
    :data-selected="selected"
    @click="pointer.point"
    @dblclick="pointer.open"
  >
    <button
      class="check"
      type="button"
      :aria-pressed="selected"
      :aria-label="`Select ${issue.id}`"
      tabindex="-1"
      @click.stop="pointer.toggle"
    ></button>
    <span
      class="priority"
      :data-priority="issue.priority"
      :title="priorityLabel(issue.priority)"
      role="img"
      :aria-label="priorityLabel(issue.priority)"
    ><i></i><i></i><i></i></span>
    <span
      class="status-icon"
      :data-category="issue.category"
      role="img"
      :aria-label="status(issue)"
      :title="status(issue)"
    ></span>
    <a class="issue-id" :href="pointer.href" tabindex="-1" @click="pointer.follow">{{ issue.id }}</a>
    <span class="issue-title">{{ issue.title }}</span>
    <IssueMeta :issue="issue" />
  </li>
</template>
