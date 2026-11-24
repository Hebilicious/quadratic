<script setup lang="ts" vapor>
import { nextTick, watch } from "vue";

import type { IssueSummary, Project } from "../board/model.ts";
import type { AgentWork, NowView } from "../board/now.ts";
import { paths } from "../route.ts";
import { useWorkspace } from "../workspace.ts";
import {
  actorDetails,
  ago,
  priorityLabel,
  when,
  worktreeBesideName,
  worktreeName,
} from "./present.ts";

const props = defineProps<{
  readonly now: NowView;
  readonly projects: readonly Project[];
  readonly state: "loading" | "ready" | "failed";
  readonly section: "agents" | "waiting";
}>();
const emit = defineEmits<{ retry: [] }>();
const { router } = useWorkspace();

const statusName = (issue: IssueSummary) =>
  props.projects
    .find((project) => project.key === issue.project)
    ?.workflow.statuses.find((status) => status.key === issue.status)?.name ?? issue.status;

function holderLabel(agent: AgentWork): string {
  const worktree = worktreeBesideName(agent.holder.name, agent.worktree);
  return worktree === undefined ? agent.holder.name : `${agent.holder.name} in ${worktree}`;
}

watch(
  () => props.section,
  async (section) => {
    await nextTick();
    document.getElementById(`now-${section}`)?.focus();
  },
  { immediate: true },
);

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
  <p v-if="state === 'failed'" class="empty">
    The now view did not load. <button class="link-button" type="button" @click="emit('retry')">Retry</button>
  </p>
  <div v-else class="now" :aria-busy="state === 'loading'">
    <section class="now-section" aria-labelledby="now-waiting">
      <h2 id="now-waiting" class="now-heading" tabindex="-1">
        Waits on you <span class="group-count">{{ now.assigned.length + now.review.length }}</span>
      </h2>
      <div class="now-columns">
        <section class="now-block" aria-label="Assigned to you">
          <h3 class="now-subheading">Assigned to you</h3>
          <ul v-if="now.assigned.length > 0" class="now-list" aria-label="Assigned to you">
            <li v-for="issue in now.assigned" :key="issue.id" class="now-item">
              <span
                class="priority"
                :data-priority="issue.priority"
                role="img"
                :aria-label="priorityLabel(issue.priority)"
              ><i></i><i></i><i></i></span>
              <a class="issue-link" :href="paths.issue(issue.id)" @click="follow">{{ issue.id }}</a>
              <span class="now-title">{{ issue.title }}</span>
              <span class="muted">{{ statusName(issue) }}</span>
            </li>
          </ul>
          <p v-else class="field-empty">Nothing is assigned to you.</p>
        </section>
        <section class="now-block" aria-label="Waiting for review or merge">
          <h3 class="now-subheading">Waiting for review or merge</h3>
          <ul v-if="now.review.length > 0" class="now-list" aria-label="Waiting for review or merge">
            <li v-for="issue in now.review" :key="issue.id" class="now-item">
              <span
                class="priority"
                :data-priority="issue.priority"
                role="img"
                :aria-label="priorityLabel(issue.priority)"
              ><i></i><i></i><i></i></span>
              <a class="issue-link" :href="paths.issue(issue.id)" @click="follow">{{ issue.id }}</a>
              <span class="now-title">{{ issue.title }}</span>
              <span v-if="issue.claim !== undefined" class="muted">
                {{ issue.claim.holder.name }} on <code>{{ issue.claim.branch }}</code>
              </span>
            </li>
          </ul>
          <p v-else class="field-empty">Nothing waits for a review or a merge.</p>
        </section>
      </div>
    </section>

    <section class="now-section" aria-labelledby="now-agents">
      <h2 id="now-agents" class="now-heading" tabindex="-1">
        What every agent is doing <span class="group-count">{{ now.agents.length }}</span>
      </h2>
      <p v-if="now.agents.length === 0" class="field-empty">No agent holds an issue.</p>
      <div v-else class="agents">
        <section
          v-for="agent in now.agents"
          :key="`${agent.holder.name} ${agent.worktree}`"
          class="agent"
          :aria-label="holderLabel(agent)"
        >
          <h3 class="agent-name">
            {{ holderLabel(agent) }}
            <span v-for="part in actorDetails(agent.holder)" :key="part" class="claim-part">{{ part }}</span>
          </h3>
          <ul class="now-list" :aria-label="`${holderLabel(agent)} holds`">
            <li v-for="issue in agent.issues" :key="issue.id" class="now-item agent-item">
              <a class="issue-link" :href="paths.issue(issue.id)" @click="follow">{{ issue.id }}</a>
              <span class="now-title">{{ issue.title }}</span>
              <span class="status-pill"><span class="status-icon" :data-category="issue.category" aria-hidden="true"></span>{{ statusName(issue) }}</span>
              <template v-if="issue.claim !== undefined">
                <span class="claim-branch">{{ issue.claim.branch }}</span>
                <span class="claim-worktree" :title="issue.claim.worktree">{{ worktreeName(issue.claim.worktree) }}</span>
                <span class="muted" :title="when(issue.claim.since)">since {{ ago(issue.claim.since) }}</span>
              </template>
            </li>
          </ul>
        </section>
      </div>
    </section>
  </div>
</template>
