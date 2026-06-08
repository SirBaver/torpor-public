CREATE TABLE IF NOT EXISTS memory (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    last_action_id TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_memory_action ON memory(last_action_id);

CREATE TABLE IF NOT EXISTS memory_history (
    history_id INTEGER PRIMARY KEY AUTOINCREMENT,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    action_id TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_history_key ON memory_history(key);
CREATE INDEX IF NOT EXISTS idx_history_timestamp ON memory_history(timestamp);

CREATE TABLE IF NOT EXISTS actions (
    action_id TEXT PRIMARY KEY,
    timestamp TEXT NOT NULL,
    type TEXT NOT NULL,
    caused_by TEXT,
    caused_by_list TEXT,
    payload TEXT,
    result TEXT,
    session_id TEXT,
    FOREIGN KEY (caused_by) REFERENCES actions(action_id)
);

CREATE INDEX IF NOT EXISTS idx_actions_type ON actions(type);
CREATE INDEX IF NOT EXISTS idx_actions_caused_by ON actions(caused_by);
CREATE INDEX IF NOT EXISTS idx_actions_timestamp ON actions(timestamp);

CREATE TABLE IF NOT EXISTS snapshots (
    snapshot_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    state_hash TEXT NOT NULL,
    action_count INTEGER NOT NULL,
    last_action_id TEXT,
    state_json TEXT,
    FOREIGN KEY (last_action_id) REFERENCES actions(action_id)
);

CREATE INDEX IF NOT EXISTS idx_snapshots_timestamp ON snapshots(timestamp);

CREATE TABLE IF NOT EXISTS capabilities (
    cap_id      TEXT PRIMARY KEY,
    parent_cap  TEXT REFERENCES capabilities(cap_id),
    subject     TEXT NOT NULL,
    op          TEXT NOT NULL,
    scope       TEXT NOT NULL,
    issued_at   TEXT NOT NULL,
    issued_by   TEXT NOT NULL,
    revoked_at  TEXT,
    revoked_by  TEXT
);

CREATE INDEX IF NOT EXISTS idx_caps_subject ON capabilities(subject);
CREATE INDEX IF NOT EXISTS idx_caps_parent ON capabilities(parent_cap);
