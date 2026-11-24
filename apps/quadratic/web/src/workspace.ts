import { inject, type InjectionKey } from "vue";
import type { Actor } from "xstate";

import type { BoardMachine } from "./board/board.machine.ts";
import type { CommandsMachine } from "./board/commands.machine.ts";
import type { SavedViewsMachine } from "./board/saved-views.machine.ts";
import type { Quadratic } from "./core/quadratic.ts";
import type { Drafts } from "./edit/drafts.ts";
import type { keysMachine } from "./keys/keys.machine.ts";
import type { createRouter } from "./route.ts";

export interface Workspace {
  readonly quadratic: Quadratic;
  readonly board: Actor<BoardMachine>;
  readonly keys: Actor<ReturnType<typeof keysMachine>>;
  readonly commands: Actor<CommandsMachine>;
  readonly savedViews: Actor<SavedViewsMachine>;
  readonly router: ReturnType<typeof createRouter>;
  readonly drafts: Drafts;
}

export const workspaceKey: InjectionKey<Workspace> = Symbol("quadratic-workspace");

export function useWorkspace(): Workspace {
  const workspace = inject(workspaceKey);
  if (workspace === undefined) {
    throw new Error("The page's workspace was not provided");
  }
  return workspace;
}
