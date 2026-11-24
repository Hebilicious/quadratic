export interface KeptDraft {
  readonly text: string;
  readonly base: number;
}

export class Drafts {
  private readonly kept = new Map<string, KeptDraft>();

  get(key: string): KeptDraft | undefined {
    return this.kept.get(key);
  }

  keep(key: string, draft: KeptDraft): void {
    this.kept.set(key, draft);
  }

  drop(key: string): void {
    this.kept.delete(key);
  }
}
