import { computed, onScopeDispose, watch } from "vue";
import { createActor } from "xstate";

import { useSnapshot } from "../use-actor.ts";
import { editMachine, editPhase } from "./edit.machine.ts";

export function useEdit(ports: Parameters<typeof editMachine>[0]) {
  const actor = createActor(editMachine(ports));
  actor.start();
  const stopWatching = watch(ports.revision, (revision) => {
    actor.send({ type: "revised", revision });
  });
  onScopeDispose(() => {
    stopWatching();
    actor.stop();
  });
  const snapshot = useSnapshot(actor);
  return {
    actor,
    phase: computed(() => editPhase(snapshot.value)),
    error: computed(() => snapshot.value.context.error),
    base: computed(() => snapshot.value.context.base),
    refreshed: computed(() => snapshot.value.matches({ stale: "refreshed" })),
  };
}
