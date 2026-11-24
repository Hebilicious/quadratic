import type { DetailActor } from "../board/detail.machine.ts";
import type { IssueSummary, RelationKind } from "../board/model.ts";
import type { IssuePatch, Quadratic } from "../core/quadratic.ts";

export interface LinkChange {
  readonly from: string;
  readonly kind: RelationKind;
  readonly target: string;
  readonly remove: boolean;
}

export interface IssueEditor {
  readonly issue: string;
  readonly revision: () => number;
  readonly refresh: () => void;
  readonly update: (patch: IssuePatch, base: number, signal: AbortSignal) => Promise<unknown>;
  readonly link: (change: LinkChange, signal: AbortSignal) => Promise<unknown>;
  readonly release: (signal: AbortSignal) => Promise<unknown>;
  readonly close: (evidence: string, signal: AbortSignal) => Promise<unknown>;
}

export function issueEditor(options: {
  readonly id: string;
  readonly detail: DetailActor;
  readonly revision: () => number;
  readonly quadratic: Pick<Quadratic, "update" | "link" | "release" | "close">;
  readonly settle: (issues: readonly IssueSummary[]) => void;
}): IssueEditor {
  const { id, detail, quadratic, settle } = options;
  return {
    issue: id,
    revision: options.revision,
    refresh: () => {
      detail.send({ type: "reload" });
    },
    update: async (patch, base, signal) => {
      settle([await quadratic.update({ issue: id, revision: base, patch }, signal)]);
    },
    link: async (change, signal) => {
      await quadratic.link(
        { issue: change.from, kind: change.kind, target: change.target, remove: change.remove },
        signal,
      );
      detail.send({ type: "reload" });
    },
    release: async (signal) => {
      settle([await quadratic.release(id, signal)]);
    },
    close: async (evidence, signal) => {
      settle([await quadratic.close({ issue: id, evidence }, signal)]);
    },
  };
}
