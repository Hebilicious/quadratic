import { onScopeDispose } from "vue";

const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(", ");

export function useModal(id: string): void {
  const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;

  function contain(event: KeyboardEvent): void {
    const dialog = document.getElementById(id);
    if (event.key !== "Tab" || dialog === null) {
      return;
    }
    const controls = [...dialog.querySelectorAll<HTMLElement>(FOCUSABLE)];
    const first = controls[0];
    const last = controls.at(-1);
    const active = document.activeElement;
    const inside = active !== null && dialog.contains(active);
    if (first === undefined || last === undefined) {
      event.preventDefault();
    } else if (event.shiftKey && (!inside || active === first)) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (!inside || active === last)) {
      event.preventDefault();
      first.focus();
    }
  }

  document.addEventListener("keydown", contain, true);
  onScopeDispose(() => {
    document.removeEventListener("keydown", contain, true);
    if (opener?.isConnected === true) {
      opener.focus();
    }
  });
}
