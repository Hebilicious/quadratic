<script setup lang="ts" vapor>
import { onMounted } from "vue";

import { keyLabel, SHORTCUTS } from "../keys/keys.ts";
import { useWorkspace } from "../workspace.ts";
import { useModal } from "./modal.ts";

const { keys } = useWorkspace();

onMounted(() => {
  document.getElementById("help-close")?.focus();
});

useModal("help-dialog");
</script>

<template>
  <div class="backdrop" @click.self="keys.send({ type: 'close' })">
    <div
      id="help-dialog"
      class="dialog help"
      role="dialog"
      aria-modal="true"
      aria-labelledby="help-title"
    >
      <div class="dialog-head">
        <h2 id="help-title" class="dialog-title">Keyboard shortcuts</h2>
        <button
          id="help-close"
          class="ghost-button"
          type="button"
          @click="keys.send({ type: 'close' })"
        >
          Close <kbd>Esc</kbd>
        </button>
      </div>
      <dl class="shortcuts">
        <template v-for="shortcut in SHORTCUTS" :key="shortcut.does">
          <dt>
            <template v-for="(key, index) in shortcut.keys" :key="key">
              <kbd>{{ keyLabel(key) }}</kbd><span v-if="index < shortcut.keys.length - 1" class="muted"> {{ shortcut.join }} </span>
            </template>
          </dt>
          <dd>{{ shortcut.does }}</dd>
        </template>
      </dl>
      <p class="muted">Shortcuts do not fire while you type in a field. In a field, <kbd>Esc</kbd> cancels.</p>
    </div>
  </div>
</template>
