<script setup lang="ts" vapor>
import { computed, nextTick, onMounted, ref, watch } from "vue";

import { rolesOf } from "../board/issues.ts";
import type { IssueSummary } from "../board/model.ts";
import type { Moment, Surface } from "../keys/keys.machine.ts";
import { targets as targetsOf } from "../keys/keys.machine.ts";
import { keyLabel } from "../keys/keys.ts";
import { matching, menuItems, type MenuItem } from "../keys/menu.ts";
import { switched } from "../route.ts";
import { useSnapshot } from "../use-actor.ts";
import { useWorkspace } from "../workspace.ts";
import { useModal } from "./modal.ts";

const props = defineProps<{
  readonly surface: Surface;
  readonly moment: Moment;
}>();
const { board, keys, commands, router } = useWorkspace();
const boardSnapshot = useSnapshot(board);
const keysSnapshot = useSnapshot(keys);

const query = ref(keys.getSnapshot().context.menuQuery);
const active = ref(0);

const targets = computed(() => {
  const { issues, now } = boardSnapshot.value.context;
  return targetsOf(keysSnapshot.value.context)
    .map((id) => issues[id] ?? now[id])
    .filter((issue): issue is IssueSummary => issue !== undefined);
});
const items = computed(() =>
  menuItems({
    projects: boardSnapshot.value.context.projects,
    view: props.surface,
    route: router.route.value,
    targets: props.surface === "other" || props.moment === "past" ? [] : targets.value,
    roles: rolesOf(boardSnapshot.value.context.issues, boardSnapshot.value.context.now),
    query: query.value,
  }),
);
const shown = computed(() => matching(items.value, query.value));
const activeItem = computed(() => shown.value[Math.min(active.value, shown.value.length - 1)]);

watch(query, () => {
  active.value = 0;
});

onMounted(() => {
  const input = document.getElementById("command-input");
  if (input instanceof HTMLInputElement) {
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
  }
});

function close(): void {
  keys.send({ type: "close" });
}

function choose(item: MenuItem): void {
  const { action } = item;
  switch (action.kind) {
    case "go":
      close();
      router.go(action.path);
      return;
    case "switchView": {
      close();
      const other = switched(router.route.value);
      if (other !== undefined) {
        router.go(other);
      }
      return;
    }
    case "create":
      keys.send({ type: "openCreate" });
      return;
    case "help":
      keys.send({ type: "openHelp" });
      return;
    case "search":
      close();
      void nextTick(() => {
        document.getElementById("filter-text")?.focus();
      });
      return;
    case "peek":
      close();
      keys.send({ type: "peekAt", issue: action.issue });
      return;
    case "run":
      close();
      commands.send({ type: "run", commands: action.commands });
      return;
    default: {
      const unhandled: never = action;
      return unhandled;
    }
  }
}

function onInput(event: Event): void {
  if (event.target instanceof HTMLInputElement) {
    query.value = event.target.value;
  }
}

function onKey(event: KeyboardEvent): void {
  const down = event.key === "ArrowDown" || (event.ctrlKey && event.key === "n");
  const up = event.key === "ArrowUp" || (event.ctrlKey && event.key === "p");
  if (down || up) {
    event.preventDefault();
    const count = shown.value.length;
    if (count > 0) {
      active.value = (Math.min(active.value, count - 1) + (down ? 1 : count - 1)) % count;
      document
        .getElementById(`command-${String(active.value)}`)
        ?.scrollIntoView({ block: "nearest" });
    }
  } else if (event.key === "Enter") {
    event.preventDefault();
    if (activeItem.value !== undefined) {
      choose(activeItem.value);
    }
  } else if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    close();
  }
}

useModal("command-menu");
</script>

<template>
  <div class="backdrop" @click.self="close">
    <div
      id="command-menu"
      class="dialog menu"
      role="dialog"
      aria-modal="true"
      aria-label="Command menu"
    >
      <input
        id="command-input"
        class="menu-input"
        role="combobox"
        aria-expanded="true"
        aria-controls="command-list"
        :aria-activedescendant="activeItem === undefined ? undefined : `command-${String(shown.indexOf(activeItem))}`"
        aria-label="Type a command"
        placeholder="Type a command or search…"
        autocomplete="off"
        :value="query"
        @input="onInput"
        @keydown="onKey"
      />
      <p v-if="targets.length > 0 && surface !== 'other'" class="menu-context">
        On {{ targets.length === 1 ? targets[0]?.id : `${targets.length} selected issues` }}
      </p>
      <ul id="command-list" class="menu-list" role="listbox" aria-label="Commands">
        <li
          v-for="(item, index) in shown"
          :id="`command-${String(index)}`"
          :key="item.id"
          class="menu-item"
          role="option"
          :aria-selected="item === activeItem"
          @click="choose(item)"
          @mousemove="active = index"
        >
          <span class="menu-group">{{ item.group }}</span>
          <span class="menu-label">{{ item.label }}</span>
          <kbd v-if="item.hint !== undefined">{{ keyLabel(item.hint) }}</kbd>
        </li>
        <li v-if="shown.length === 0" class="menu-empty">No command matches.</li>
      </ul>
    </div>
  </div>
</template>
