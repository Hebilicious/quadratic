CREATE TABLE IF NOT EXISTS saved_views (
  id       TEXT PRIMARY KEY,
  name     TEXT NOT NULL,
  project  TEXT,
  filter   TEXT NOT NULL,
  position INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS saved_view_state (
  next_id  INTEGER NOT NULL,
  revision INTEGER NOT NULL
);
INSERT INTO saved_view_state (next_id, revision)
  SELECT 1, 0 WHERE NOT EXISTS (SELECT 1 FROM saved_view_state);

CREATE TABLE IF NOT EXISTS saved_view_imports (
  source TEXT NOT NULL,
  old    TEXT NOT NULL,
  view   TEXT NOT NULL,
  PRIMARY KEY (source, old)
);
