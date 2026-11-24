import { onScopeDispose, shallowRef, type ShallowRef } from "vue";

interface Observed<Snapshot> {
  getSnapshot(): Snapshot;
  subscribe(listener: (snapshot: Snapshot) => void): { unsubscribe(): void };
}

export function useSnapshot<Snapshot>(source: Observed<Snapshot>): Readonly<ShallowRef<Snapshot>> {
  const snapshot = shallowRef(source.getSnapshot());
  const subscription = source.subscribe((next) => {
    snapshot.value = next;
  });
  onScopeDispose(() => {
    subscription.unsubscribe();
  });
  return snapshot;
}
