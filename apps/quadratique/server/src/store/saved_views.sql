CREATE TABLE IF NOT EXISTS saved_views (
  id       TEXT PRIMARY KEY,
  name     TEXT NOT NULL,
  project  TEXT,
  filter   TEXT NOT NULL,
  position INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS saved_view_numbers (
  next INTEGER NOT NULL
);
INSERT INTO saved_view_numbers (next) SELECT 1 WHERE NOT EXISTS (SELECT 1 FROM saved_view_numbers);
