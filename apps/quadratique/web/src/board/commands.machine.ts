import { createAsyncLogic, setup, types } from "xstate";

import type { IssuePatch, Quadratique } from "../core/quadratique.ts";
import type { IssueSummary } from "./model.ts";
import { plannedPatch, type ShiftPlan } from "./view.ts";

export type Command =
  | {
      readonly kind: "update";
      readonly issue: string;
      readonly revision: number;
      readonly patch: IssuePatch;
    }
  | {
      readonly kind: "shift";
      readonly issue: string;
      readonly by: -1 | 1;
      readonly plan: ShiftPlan;
    }
  | { readonly kind: "release"; readonly issue: string };

type Resolved = Exclude<Command, { kind: "shift" }>;

export interface Failed {
  readonly issue: string;
  readonly error: unknown;
}

export interface CommandPorts {
  readonly quadratique: Pick<Quadratique, "update" | "release">;
  readonly current: (issue: string) => IssueSummary | undefined;
  readonly settle: (issues: readonly IssueSummary[]) => void;
}

function resolved(given: readonly Command[], unsent: readonly Resolved[]): readonly Resolved[] {
  const out: Resolved[] = [];
  for (const command of given) {
    if (command.kind !== "shift") {
      out.push(command);
      continue;
    }
    const { revision } = command.plan.issue;
    const pending = [...unsent, ...out].flatMap((each) =>
      each.kind === "update" && each.issue === command.issue && each.revision === revision
        ? [each.patch]
        : [],
    );
    const patch = plannedPatch(command.plan, command.by, pending);
    if (patch !== undefined) {
      out.push({ kind: "update", issue: command.issue, revision, patch });
    }
  }
  return out;
}

async function runOne(
  ports: CommandPorts,
  command: Resolved,
  signal: AbortSignal,
): Promise<Failed | undefined> {
  const issue = ports.current(command.issue);
  if (issue === undefined) {
    return { issue: command.issue, error: new Error("The issue is not on the board") };
  }
  try {
    const updated =
      command.kind === "release"
        ? await ports.quadratique.release(issue.id, signal)
        : await ports.quadratique.update(
            { issue: issue.id, revision: issue.revision, patch: command.patch },
            signal,
          );
    ports.settle([updated]);
    return undefined;
  } catch (error) {
    return { issue: issue.id, error };
  }
}

export function commandsMachine(ports: CommandPorts) {
  const send = createAsyncLogic<Failed | undefined, { readonly command: Resolved }>({
    run: async ({ input, signal }) => await runOne(ports, input.command, signal),
  });

  const settled = (
    queue: readonly Resolved[],
    failed: readonly Failed[],
  ):
    | {
        target: "running";
        reenter: true;
        context: { queue: readonly Resolved[]; failed: readonly Failed[] };
      }
    | {
        target: "idle" | "reported";
        context: { queue: readonly Resolved[]; failed: readonly Failed[] };
      } =>
    queue.length > 0
      ? { target: "running", reenter: true, context: { queue, failed } }
      : { target: failed.length > 0 ? "reported" : "idle", context: { queue, failed } };

  return setup({
    schemas: {
      context: types<{
        readonly queue: readonly Resolved[];
        readonly failed: readonly Failed[];
      }>(),
      events: {
        run: types<{ commands: readonly Command[] }>(),
        past: types<void>(),
        dismiss: types<void>(),
      },
    },
    actors: { send },
  }).createMachine({
    id: "commands",
    context: { queue: [], failed: [] },
    initial: "idle",
    states: {
      idle: {
        on: {
          run: ({ event }) => {
            const queue = resolved(event.commands, []);
            return queue.length > 0 ? { target: "running", context: { queue } } : undefined;
          },
        },
      },
      running: {
        invoke: {
          src: "send",
          input: ({ context }) => {
            const [command] = context.queue;
            if (command === undefined) {
              throw new Error("The commands ran with nothing to send");
            }
            return { command };
          },
          onDone: ({ context, event }) =>
            settled(
              context.queue.slice(1),
              event.output === undefined ? context.failed : [...context.failed, event.output],
            ),
          onError: ({ context, event }) =>
            settled(context.queue.slice(1), [...context.failed, { issue: "", error: event.error }]),
        },
        on: {
          run: ({ context, event }) => ({
            context: { queue: [...context.queue, ...resolved(event.commands, context.queue)] },
          }),
          past: ({ context }) => ({ context: { queue: context.queue.slice(0, 1) } }),
        },
      },
      reported: {
        on: {
          run: ({ event }) => {
            const queue = resolved(event.commands, []);
            return queue.length > 0
              ? { target: "running", context: { queue, failed: [] } }
              : undefined;
          },
          dismiss: { target: "idle", context: { failed: [] } },
        },
      },
    },
  });
}

export type CommandsMachine = ReturnType<typeof commandsMachine>;
