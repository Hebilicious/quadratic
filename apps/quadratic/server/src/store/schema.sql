CREATE TABLE IF NOT EXISTS projects (
  key            TEXT PRIMARY KEY,
  name           TEXT NOT NULL,
  repository     TEXT UNIQUE,
  default_branch TEXT NOT NULL,
  moves          TEXT NOT NULL,
  extra          TEXT NOT NULL DEFAULT '{}',
  next_number    INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS statuses (
  project  TEXT NOT NULL,
  key      TEXT NOT NULL,
  name     TEXT NOT NULL,
  category TEXT NOT NULL CHECK (category IN ('backlog', 'unstarted', 'started', 'completed', 'canceled')),
  position INTEGER NOT NULL,
  PRIMARY KEY (project, key)
);

CREATE TABLE IF NOT EXISTS issues (
  id             TEXT PRIMARY KEY,
  project        TEXT NOT NULL,
  number         INTEGER NOT NULL,
  title          TEXT NOT NULL,
  status         TEXT NOT NULL,
  priority       INTEGER NOT NULL DEFAULT 0,
  assignee       TEXT,
  parent         TEXT,
  milestone      TEXT,
  sort_order     REAL NOT NULL,
  due            TEXT,
  description    TEXT,
  acceptance     TEXT,
  notes          TEXT,
  evidence       TEXT,
  claim_holder   TEXT,
  claim_key      TEXT,
  claim_branch   TEXT,
  claim_worktree TEXT,
  claim_since    TEXT,
  revision       INTEGER NOT NULL DEFAULT 1,
  client_key     TEXT UNIQUE,
  client_request TEXT,
  created_by     TEXT NOT NULL,
  created_at     TEXT NOT NULL,
  updated_at     TEXT NOT NULL,
  UNIQUE (project, number)
);
CREATE INDEX IF NOT EXISTS issues_by_order ON issues (sort_order, project, number);
CREATE INDEX IF NOT EXISTS issues_by_parent ON issues (parent);

CREATE TABLE IF NOT EXISTS notes (
  issue  TEXT NOT NULL,
  number INTEGER NOT NULL,
  text   TEXT NOT NULL,
  author TEXT NOT NULL,
  at     TEXT NOT NULL,
  PRIMARY KEY (issue, number)
);

INSERT INTO notes (issue, number, text, author, at)
  SELECT id, 1, notes, '{"harness":"quadratic","kind":"agent","name":"quadratic"}', updated_at
  FROM issues WHERE notes IS NOT NULL;
UPDATE issues SET notes = NULL WHERE notes IS NOT NULL;

CREATE TABLE IF NOT EXISTS labels (
  issue TEXT NOT NULL,
  label TEXT NOT NULL,
  PRIMARY KEY (issue, label)
);
CREATE INDEX IF NOT EXISTS labels_by_label ON labels (label);

CREATE TABLE IF NOT EXISTS relations (
  source TEXT NOT NULL,
  kind   TEXT NOT NULL CHECK (kind IN ('blocks', 'related', 'duplicate', 'discovered-from', 'stacked-on')),
  target TEXT NOT NULL,
  PRIMARY KEY (source, kind, target)
);
CREATE INDEX IF NOT EXISTS relations_by_target ON relations (target);

CREATE TABLE IF NOT EXISTS sources (
  issue    TEXT PRIMARY KEY,
  project  TEXT NOT NULL,
  tool     TEXT NOT NULL,
  key      TEXT NOT NULL,
  decision INTEGER NOT NULL DEFAULT 0,
  finished_evidence TEXT,
  UNIQUE (project, tool, key)
);

CREATE TABLE IF NOT EXISTS finding_links (
  issue TEXT NOT NULL,
  kind  TEXT NOT NULL,
  id    TEXT NOT NULL,
  PRIMARY KEY (issue, kind, id)
);

CREATE TABLE IF NOT EXISTS finding_blocks (
  source TEXT NOT NULL,
  target TEXT NOT NULL,
  PRIMARY KEY (source, target)
);

CREATE TABLE IF NOT EXISTS git_landings (
  issue      TEXT NOT NULL,
  git_commit TEXT NOT NULL,
  project    TEXT NOT NULL,
  branch     TEXT,
  landed_in  TEXT NOT NULL,
  PRIMARY KEY (issue, git_commit)
);

CREATE TABLE IF NOT EXISTS held_merges (
  issue TEXT PRIMARY KEY
);

CREATE TABLE IF NOT EXISTS git_watch (
  project        TEXT PRIMARY KEY,
  repository     TEXT NOT NULL,
  default_branch TEXT NOT NULL,
  tip            TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS git_upstream (
  project TEXT PRIMARY KEY,
  tip     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS git_work (
  project TEXT NOT NULL,
  branch  TEXT NOT NULL,
  tip     TEXT NOT NULL,
  PRIMARY KEY (project, branch)
);

CREATE TABLE IF NOT EXISTS claim_starts (
  issue      TEXT PRIMARY KEY,
  claim_key  TEXT NOT NULL,
  since      TEXT NOT NULL,
  repository TEXT NOT NULL,
  tip        TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS commit_index (
  seq     INTEGER PRIMARY KEY,
  hash    TEXT NOT NULL UNIQUE,
  parent  TEXT NOT NULL,
  at      TEXT NOT NULL,
  message TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS commit_subjects (
  seq     INTEGER NOT NULL,
  project TEXT NOT NULL,
  issue   TEXT NOT NULL,
  PRIMARY KEY (seq, project, issue)
);
CREATE INDEX IF NOT EXISTS commit_subjects_by_issue ON commit_subjects (issue, seq);
CREATE INDEX IF NOT EXISTS commit_subjects_by_project ON commit_subjects (project, seq);

INSERT OR IGNORE INTO dolt_ignore (pattern, ignored) VALUES ('commit_index', 1), ('commit_subjects', 1),
  ('git_watch', 1), ('git_upstream', 1), ('git_work', 1), ('claim_starts', 1);
