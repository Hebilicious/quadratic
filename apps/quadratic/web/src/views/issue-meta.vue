<script setup lang="ts" vapor>
import type { IssueSummary } from "../board/model.ts";
import { assigneeLabel, worktreeBesideName } from "./present.ts";

defineProps<{ readonly issue: IssueSummary }>();
</script>

<template>
  <span
    v-if="issue.labels.length > 0 || issue.claim !== undefined || issue.assignee !== undefined"
    class="meta"
  >
    <span v-for="label in issue.labels" :key="label" class="label">{{ label }}</span>
    <span
      v-if="issue.claim !== undefined"
      class="claim"
      :title="`${issue.claim.holder.name} holds it on ${issue.claim.branch} in ${issue.claim.worktree}`"
    >
      <span class="claim-holder">{{ issue.claim.holder.name }}</span>
      <span class="claim-branch">{{ issue.claim.branch }}</span>
      <span
        v-if="worktreeBesideName(issue.claim.holder.name, issue.claim.worktree) !== undefined"
        class="claim-worktree"
      >{{ worktreeBesideName(issue.claim.holder.name, issue.claim.worktree) }}</span>
    </span>
    <span
      v-if="assigneeLabel(issue.assignee) !== undefined"
      class="assignee"
      :data-kind="issue.assignee?.kind"
    >{{ assigneeLabel(issue.assignee) }}</span>
  </span>
</template>
