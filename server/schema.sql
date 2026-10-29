-- PackLink D1-Schema
CREATE TABLE IF NOT EXISTS users (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  username   TEXT NOT NULL UNIQUE COLLATE NOCASE,
  pw_hash    TEXT NOT NULL,
  pw_salt    TEXT NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
  token_hash TEXT PRIMARY KEY,
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);

-- user_a < user_b, eine Zeile pro Paar
CREATE TABLE IF NOT EXISTS friendships (
  user_a     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  user_b     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  requester  INTEGER NOT NULL,
  status     TEXT NOT NULL CHECK (status IN ('pending','accepted')),
  created_at INTEGER NOT NULL,
  PRIMARY KEY (user_a, user_b)
);

CREATE TABLE IF NOT EXISTS packs (
  id             TEXT PRIMARY KEY,
  owner_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name           TEXT NOT NULL,
  description    TEXT NOT NULL DEFAULT '',
  mc_version     TEXT NOT NULL,
  loader         TEXT NOT NULL,
  latest_version INTEGER NOT NULL DEFAULT 0,
  created_at     INTEGER NOT NULL,
  updated_at     INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS pack_members (
  pack_id   TEXT NOT NULL REFERENCES packs(id) ON DELETE CASCADE,
  user_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  role      TEXT NOT NULL CHECK (role IN ('owner','editor','member')),
  joined_at INTEGER NOT NULL,
  PRIMARY KEY (pack_id, user_id)
);
CREATE INDEX IF NOT EXISTS idx_members_user ON pack_members(user_id);

CREATE TABLE IF NOT EXISTS pack_versions (
  pack_id    TEXT NOT NULL REFERENCES packs(id) ON DELETE CASCADE,
  version    INTEGER NOT NULL,
  manifest   TEXT NOT NULL,
  changelog  TEXT NOT NULL DEFAULT '',
  author_id  INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (pack_id, version)
);

CREATE TABLE IF NOT EXISTS share_links (
  code       TEXT PRIMARY KEY,
  pack_id    TEXT NOT NULL REFERENCES packs(id) ON DELETE CASCADE,
  created_by INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  expires_at INTEGER,
  max_uses   INTEGER,
  uses       INTEGER NOT NULL DEFAULT 0
);
