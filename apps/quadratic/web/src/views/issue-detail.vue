<script setup lang="ts" vapor>
import { computed } from "vue";

import type { DetailActor } from "../board/detail.machine.ts";
import { rolesOf } from "../board/issues.ts";
import type { Assignee, Issue } from "../board/model.ts";
import { closeChanges, explainsNotReady } from "../board/lifecycle.ts";
import ChoiceField from "../edit/choice-field.vue";
import CloseForm from "../edit/close-form.vue";
import { issueEditor } from "../edit/issue-editor.ts";
import NotesField from "../edit/notes-field.vue";
import ReleaseButton from "../edit/release-button.vue";
import RelationsField from "../edit/relations-field.vue";
import TextField from "../edit/text-field.vue";
import type { IssuePatch } from "../core/quadratic.ts";
import { paths } from "../route.ts";
import { useSnapshot } from "../use-actor.ts";
import { useWorkspace } from "../workspace.ts";
import HistoryList from "./history-list.vue";
import {
  actorDetails,
  ago,
  labelsOf,
  notReadyText,
  priorityLabel,
  PRIORITY_CHOICES,
  when,
} from "./present.ts";

const props = defineProps<{
  readonly detail: DetailActor;
  readonly closing: boolean;
  readonly mode: "page" | "peek";
}>();

const { board, quadratic, router } = useWorkspace();
const boardSnapshot = useSnapshot(board);
const snapshot = useSnapshot(props.detail);
const issue = computed((): Issue | undefined => snapshot.value.context.issue);
const issueId = computed((): string => snapshot.value.context.id);
const state = computed(() =>
  snapshot.value.matches({ issue: "missing" })
    ? "missing"
    : snapshot.value.matches({ issue: "failed" })
      ? "failed"
      : snapshot.value.matches({ issue: "ready" })
        ? "ready"
        : "loading",
);
const outdated = computed(() => snapshot.value.matches({ issue: { ready: "outdated" } }));
const project = computed(() =>
  boardSnapshot.value.context.projects.find((each) => each.key === issue.value?.project),
);
const others = computed(() =>
  Object.values(boardSnapshot.value.context.issues)
    .filter((each) => each.id !== issue.value?.id)
    .toSorted((left, right) => left.id.localeCompare(right.id, undefined, { numeric: true })),
);
const roles = computed(() =>
  rolesOf(boardSnapshot.value.context.issues, boardSnapshot.value.context.now),
);

const editor = issueEditor({
  id: issueId.value,
  detail: props.detail,
  revision: () => issue.value?.revision ?? 0,
  quadratic,
  settle: (issues) => {
    board.send({ type: "settled", issues });
  },
});

const statusOptions = computed(() =>
  (project.value?.workflow.statuses ?? []).map((status) => ({
    value: status.key,
    label: status.name,
  })),
);
const priorityOptions = PRIORITY_CHOICES.map((priority) => ({
  value: String(priority),
  label: priorityLabel(priority),
}));
const assigneeValue = (assignee: Assignee | undefined): string =>
  assignee === undefined ? "" : assignee.kind === "human" ? "human" : `role:${assignee.role}`;
const assigneeOptions = computed(() => {
  const current = issue.value?.assignee;
  const known =
    current?.kind === "agent" && !roles.value.includes(current.role)
      ? [...roles.value, current.role]
      : roles.value;
  return [
    { value: "", label: "Nobody" },
    { value: "human", label: "You" },
    ...known.map((role) => ({ value: `role:${role}`, label: `Role ${role}` })),
  ];
});
const parentOptions = computed(() => [
  { value: "", label: "None" },
  ...others.value.map((each) => ({ value: each.id, label: `${each.id} ${each.title}` })),
  ...(issue.value?.parent !== undefined &&
  !others.value.some((each) => each.id === issue.value?.parent)
    ? [{ value: issue.value.parent, label: issue.value.parent }]
    : []),
]);

const saveStatus = (value: string): IssuePatch => ({ status: value });
const savePriority = (value: string): IssuePatch => ({ priority: Number(value) });
const saveAssignee = (value: string): IssuePatch =>
  value === ""
    ? { clear: ["assignee"] }
    : value === "human"
      ? { assignee: { kind: "human" } }
      : { assignee: { kind: "agent", role: value.slice("role:".length) } };
const saveParent = (value: string): IssuePatch =>
  value === "" ? { clear: ["parent"] } : { parent: value };
const saveTitle = (text: string): IssuePatch => ({ title: text });
const saveLabels = (text: string): IssuePatch => ({ labels: labelsOf(text) });
const saveDescription = (text: string): IssuePatch =>
  text.length === 0 ? { clear: ["description"] } : { description: text };
const saveAcceptance = (text: string): IssuePatch => ({ acceptance: text });

const children = computed(() => {
  const all = boardSnapshot.value.context.issues;
  return (issue.value?.children ?? []).map((id) => ({ id, summary: all[id] }));
});
const statusName = (key: string) =>
  project.value?.workflow.statuses.find((status) => status.key === key)?.name ?? key;

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
  <p v-if="state === 'loading'" class="empty" aria-busy="true">Loading the issue…</p>
  <p v-else-if="state === 'missing'" class="empty">There is no issue {{ issueId }}.</p>
  <p v-else-if="state === 'failed' || issue === undefined" class="empty">
    The issue did not load.
    <button class="link-button" type="button" @click="detail.send({ type: 'retry' })">Retry</button>
  </p>
  <article v-else class="issue" :data-mode="mode" :aria-label="`${issue.id} ${issue.title}`">
    <header class="issue-head">
      <p class="issue-kicker">
        <span class="issue-id">{{ issue.id }}</span>
        <span class="status-icon" :data-category="issue.category" aria-hidden="true"></span>
        <span>{{ statusName(issue.status) }}</span>
      </p>
      <TextField
        label="Title"
        :value="issue.title"
        :editor="editor"
        :save="saveTitle"
        :multiline="false"
        :required="true"
        empty=""
      />
    </header>
    <div class="issue-main">
      <p v-if="outdated" class="field-error" role="alert">
        The issue changed, and it did not load again: what you see may be out of date.
        <button class="link-button" type="button" @click="detail.send({ type: 'retry' })">Load it again</button>
      </p>

      <section
        v-if="explainsNotReady(issue) && issue.notReady !== undefined"
        class="readiness"
        aria-label="Why it is not ready"
      >
        <h3 class="field-label">Not ready</h3>
        <ul class="reasons">
          <li v-for="reason in issue.notReady" :key="reason">{{ notReadyText(reason) }}</li>
        </ul>
        <p v-if="issue.blockedBy.length > 0" class="blocked-by">
          Waits on
          <a
            v-for="blocker in issue.blockedBy"
            :key="blocker"
            class="issue-link"
            :href="paths.issue(blocker)"
            @click="follow"
          >{{ blocker }}</a>
        </p>
      </section>
      <p v-else-if="issue.ready" class="readiness ready">Ready to start.</p>

      <TextField
        label="Description"
        :value="issue.description"
        :editor="editor"
        :save="saveDescription"
        :multiline="true"
        :required="false"
        empty="No description."
      />
      <TextField
        label="Acceptance criteria"
        :value="issue.acceptance"
        :editor="editor"
        :save="saveAcceptance"
        :multiline="true"
        :required="true"
        empty="No acceptance criteria."
      />
      <NotesField :notes="issue.notes" :editor="editor" />
      <section class="field">
        <h3 class="field-label">Evidence</h3>
        <p v-if="issue.evidence !== undefined" class="field-value" data-multiline="true">{{ issue.evidence }}</p>
        <p v-else class="field-value field-empty">No evidence yet: it comes with the close.</p>
      </section>
      <CloseForm v-if="closeChanges(issue, project)" :editor="editor" :focus="closing" />

      <HistoryList :detail="detail" :status-name="statusName" />
    </div>

    <aside class="issue-side" aria-label="Properties">
      <ChoiceField
        label="Status"
        :value="issue.status"
        :options="statusOptions"
        :editor="editor"
        :save="saveStatus"
      />
      <ChoiceField
        label="Priority"
        :value="String(issue.priority)"
        :options="priorityOptions"
        :editor="editor"
        :save="savePriority"
      />
      <ChoiceField
        label="Assignee"
        :value="assigneeValue(issue.assignee)"
        :options="assigneeOptions"
        :editor="editor"
        :save="saveAssignee"
      />
      <ChoiceField
        label="Parent"
        :value="issue.parent ?? ''"
        :options="parentOptions"
        :editor="editor"
        :save="saveParent"
      />
      <TextField
        label="Labels"
        :value="issue.labels.length === 0 ? undefined : issue.labels.join(', ')"
        :editor="editor"
        :save="saveLabels"
        :multiline="false"
        :required="false"
        empty="No labels."
      />

      <section class="side-section" aria-label="Claim">
        <h3 class="field-label">Claim</h3>
        <template v-if="issue.claim !== undefined">
          <dl class="claim-card">
            <dt>Holder</dt>
            <dd>
              <span class="claim-holder">{{ issue.claim.holder.name }}</span>
              <span v-for="part in actorDetails(issue.claim.holder)" :key="part" class="claim-part">{{ part }}</span>
            </dd>
            <dt>Branch</dt>
            <dd><code>{{ issue.claim.branch }}</code></dd>
            <dt>Worktree</dt>
            <dd><code class="wrap">{{ issue.claim.worktree }}</code></dd>
            <dt>Since</dt>
            <dd :title="when(issue.claim.since)">{{ ago(issue.claim.since) }}</dd>
          </dl>
          <ReleaseButton :editor="editor" :holder="issue.claim.holder.name" />
        </template>
        <p v-else class="field-empty">Nobody holds it.</p>
      </section>

      <RelationsField :issue="issue" :others="others" :editor="editor" :status-name="statusName" />

      <section v-if="issue.source !== undefined" class="side-section" aria-label="Source">
        <h3 class="field-label">Source</h3>
        <p class="source-line">
          <span class="source-tool">{{ issue.source.tool }}</span>
          <code class="wrap">{{ issue.source.key }}</code>
        </p>
        <p class="muted">The next complete run of {{ issue.source.tool }} finishes it when the finding is gone.</p>
        <ul v-if="issue.links.length > 0" class="links source-links" aria-label="Links">
          <li v-for="link in issue.links" :key="`${link.kind} ${link.id}`">
            <span class="muted">{{ link.kind }}</span>
            <code class="wrap">{{ link.id }}</code>
          </li>
        </ul>
      </section>

      <section class="side-section" aria-label="Children">
        <h3 class="field-label">Children</h3>
        <ul v-if="children.length > 0" class="links">
          <li v-for="child in children" :key="child.id">
            <a class="issue-link" :href="paths.issue(child.id)" @click="follow">{{ child.id }}</a>
            <span v-if="child.summary !== undefined">{{ child.summary.title }}</span>
            <span v-if="child.summary !== undefined" class="muted">{{ statusName(child.summary.status) }}</span>
          </li>
        </ul>
        <p v-else class="field-empty">No children.</p>
      </section>

      <p class="created muted">
        Created by {{ issue.createdBy.name }} <span :title="when(issue.createdAt)">{{ ago(issue.createdAt) }}</span>
      </p>
    </aside>
  </article>
</template>
