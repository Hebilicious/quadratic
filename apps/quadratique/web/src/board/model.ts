import { Schema } from "effect";

export const CategorySchema = Schema.Literals([
  "backlog",
  "unstarted",
  "started",
  "completed",
  "canceled",
]);
export type Category = typeof CategorySchema.Type;

export const ActorSchema = Schema.Struct({
  kind: Schema.Literals(["human", "agent"]),
  name: Schema.String,
  role: Schema.optionalKey(Schema.String),
  harness: Schema.optionalKey(Schema.String),
  host: Schema.optionalKey(Schema.String),
});
export type Actor = typeof ActorSchema.Type;

export const ClaimSchema = Schema.Struct({
  holder: ActorSchema,
  branch: Schema.String,
  worktree: Schema.String,
  since: Schema.String,
});
export type Claim = typeof ClaimSchema.Type;

export const AssigneeSchema = Schema.Union([
  Schema.Struct({ kind: Schema.Literal("human") }),
  Schema.Struct({ kind: Schema.Literal("agent"), role: Schema.String }),
]);
export type Assignee = typeof AssigneeSchema.Type;

export const NotReadyReasonSchema = Schema.Literals([
  "category",
  "blocked",
  "children",
  "stacked",
  "assigned",
  "claimed",
  "mine",
]);
export type NotReadyReason = typeof NotReadyReasonSchema.Type;

export const IssueSummarySchema = Schema.Struct({
  id: Schema.String,
  project: Schema.String,
  title: Schema.String,
  status: Schema.String,
  category: CategorySchema,
  priority: Schema.Int,
  labels: Schema.Array(Schema.String),
  assignee: Schema.optionalKey(AssigneeSchema),
  claim: Schema.optionalKey(ClaimSchema),
  parent: Schema.optionalKey(Schema.String),
  ready: Schema.Boolean,
  notReady: Schema.optionalKey(Schema.Array(NotReadyReasonSchema)),
  blockedBy: Schema.Array(Schema.String),
  order: Schema.Finite,
  revision: Schema.Int,
});
export type IssueSummary = typeof IssueSummarySchema.Type;

export const IssuePageSchema = Schema.Struct({
  items: Schema.Array(IssueSummarySchema),
  nextCursor: Schema.optionalKey(Schema.String),
});

export const RelationKindSchema = Schema.Literals([
  "blocks",
  "related",
  "duplicate",
  "discovered-from",
  "stacked-on",
]);
export type RelationKind = typeof RelationKindSchema.Type;

export const RelationSchema = Schema.Struct({
  kind: RelationKindSchema,
  direction: Schema.Literals(["outgoing", "incoming"]),
  issue: Schema.String,
  title: Schema.String,
  status: Schema.String,
  category: CategorySchema,
});
export type Relation = typeof RelationSchema.Type;

export const NoteSchema = Schema.Struct({
  text: Schema.String,
  author: Schema.String,
  at: Schema.String,
});
export type Note = typeof NoteSchema.Type;

export const SourceSchema = Schema.Struct({
  tool: Schema.String,
  key: Schema.String,
});
export type Source = typeof SourceSchema.Type;

export const LinkSchema = Schema.Struct({
  kind: Schema.String,
  id: Schema.String,
});
export type Link = typeof LinkSchema.Type;

export const LandedCommitSchema = Schema.Struct({
  commit: Schema.String,
  url: Schema.optionalKey(Schema.String),
});
export type LandedCommit = typeof LandedCommitSchema.Type;

export const WorkSchema = Schema.Struct({
  branch: Schema.optionalKey(Schema.String),
  pullRequest: Schema.optionalKey(Schema.String),
  commits: Schema.Array(LandedCommitSchema),
});
export type Work = typeof WorkSchema.Type;

export const IssueSchema = Schema.Struct({
  ...IssueSummarySchema.fields,
  description: Schema.optionalKey(Schema.String),
  acceptance: Schema.optionalKey(Schema.String),
  notes: Schema.Array(NoteSchema),
  milestone: Schema.optionalKey(Schema.String),
  evidence: Schema.optionalKey(Schema.String),
  relations: Schema.Array(RelationSchema),
  children: Schema.Array(Schema.String),
  createdAt: Schema.String,
  createdBy: ActorSchema,
  updatedAt: Schema.String,
  source: Schema.optionalKey(SourceSchema),
  links: Schema.Array(LinkSchema),
  work: Schema.optionalKey(WorkSchema),
});
export type Issue = typeof IssueSchema.Type;

export const FieldChangeSchema = Schema.Struct({
  field: Schema.String,
  before: Schema.optionalKey(Schema.String),
  after: Schema.optionalKey(Schema.String),
});
export type FieldChange = typeof FieldChangeSchema.Type;

export const HistoryChangeSchema = Schema.Struct({
  commit: Schema.String,
  at: Schema.String,
  author: ActorSchema,
  operation: Schema.String,
  project: Schema.String,
  issue: Schema.optionalKey(Schema.String),
  branch: Schema.optionalKey(Schema.String),
  worktree: Schema.optionalKey(Schema.String),
  reason: Schema.optionalKey(Schema.String),
  fields: Schema.Array(FieldChangeSchema),
});
export type HistoryChange = typeof HistoryChangeSchema.Type;

export const HistoryPageSchema = Schema.Struct({
  items: Schema.Array(HistoryChangeSchema),
  nextCursor: Schema.optionalKey(Schema.String),
});
export type HistoryPage = typeof HistoryPageSchema.Type;

export const WorkflowStatusSchema = Schema.Struct({
  key: Schema.String,
  name: Schema.String,
  category: CategorySchema,
});
export type WorkflowStatus = typeof WorkflowStatusSchema.Type;

export const ProjectSchema = Schema.Struct({
  key: Schema.String,
  name: Schema.String,
  workflow: Schema.Struct({
    statuses: Schema.Array(WorkflowStatusSchema),
    moves: Schema.Struct({
      claim: Schema.String,
      review: Schema.String,
      merge: Schema.String,
      release: Schema.String,
    }),
  }),
});
export type Project = typeof ProjectSchema.Type;

export const ProjectListSchema = Schema.Struct({ items: Schema.Array(ProjectSchema) });

const SavedViewSchema = Schema.Struct({
  id: Schema.String,
  name: Schema.String,
  project: Schema.optionalKey(Schema.String),
  filter: Schema.String,
});

export const SavedViewsSchema = Schema.Struct({
  revision: Schema.Int,
  items: Schema.Array(SavedViewSchema),
});

export const ViewsImportedSchema = Schema.Struct({
  revision: Schema.Int,
  items: Schema.Array(SavedViewSchema),
  full: Schema.Array(Schema.String),
});

export const IssueChangedSchema = Schema.Struct({
  commit: Schema.String,
  at: Schema.String,
  operation: Schema.String,
  author: ActorSchema,
  issues: Schema.Array(Schema.Struct({ issue: IssueSummarySchema })),
  projects: Schema.Array(Schema.String),
  cut: Schema.optionalKey(Schema.Literal(true)),
});
export type IssueChanged = typeof IssueChangedSchema.Type;

export const IssueAnswerSchema = Schema.Struct({ issue: IssueSchema });
export const IssueSummaryAnswerSchema = Schema.Struct({ issue: IssueSummarySchema });
export const IssueRelationsSchema = Schema.Struct({
  issue: IssueSummarySchema,
  relations: Schema.Array(RelationSchema),
});

export const FailureSchema = Schema.Union([
  Schema.Struct({ _tag: Schema.Literal("Stale"), revision: Schema.Int }),
  Schema.Struct({ _tag: Schema.Literal("Held"), issue: Schema.String, claim: ClaimSchema }),
  Schema.Struct({ _tag: Schema.Literal("Cycle"), path: Schema.Array(Schema.String) }),
  Schema.Struct({ _tag: Schema.Literal("HumanOnly") }),
  Schema.Struct({
    _tag: Schema.Literal("NotFound"),
    kind: Schema.Literals(["issue", "project", "view"]),
    id: Schema.String,
  }),
  Schema.Struct({
    _tag: Schema.Literal("UnknownStatus"),
    status: Schema.String,
    statuses: Schema.Array(Schema.String),
  }),
  Schema.Struct({ _tag: Schema.Literal("SelfLink") }),
  Schema.Struct({ _tag: Schema.Literal("NotClaimed"), issue: Schema.String }),
  Schema.Struct({ _tag: Schema.Literal("Resolved"), issue: Schema.String, status: Schema.String }),
  Schema.Struct({ _tag: Schema.Literal("KeyConflict"), issue: Schema.String }),
  Schema.Struct({ _tag: Schema.Literal("NoProject") }),
  Schema.Struct({ _tag: Schema.Literal("UnknownCommit"), commit: Schema.String }),
]);
export type Failure = typeof FailureSchema.Type;
