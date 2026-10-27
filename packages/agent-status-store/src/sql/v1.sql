
CREATE TABLE IF NOT EXISTS environments (
    id                TEXT PRIMARY KEY,
    label             TEXT NOT NULL,
    hostname          TEXT NOT NULL,
    platform          TEXT NOT NULL CHECK (platform IN ('darwin','linux')),
    user              TEXT NOT NULL,
    dropped_frames    INTEGER NOT NULL DEFAULT 0,
    last_heartbeat_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS runs (
    id              TEXT PRIMARY KEY,
    agent_id        TEXT NOT NULL,
    pid             INTEGER NOT NULL,
    harness_version TEXT NOT NULL,
    started_at      INTEGER NOT NULL,
    ended_at        INTEGER
);

CREATE TABLE IF NOT EXISTS agents (
    id                    TEXT PRIMARY KEY,
    session_id            TEXT NOT NULL,
    environment_id        TEXT NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    harness               TEXT NOT NULL CHECK (harness IN ('claude-code','pi','codex','opencode')),
    project_id            TEXT NOT NULL,
    project_name          TEXT NOT NULL,
    canonical_root        TEXT NOT NULL,
    branch                TEXT NOT NULL,
    is_detached           INTEGER NOT NULL DEFAULT 0,
    is_worktree           INTEGER NOT NULL DEFAULT 0,
    worktree_rel_path     TEXT,
    cwd                   TEXT NOT NULL,
    current_run_id        TEXT REFERENCES runs(id) ON DELETE SET NULL,
    activity              TEXT NOT NULL CHECK (activity IN ('BUSY','WAITING_QUESTION','ERROR','IDLE')),
    title                 TEXT NOT NULL DEFAULT '',
    action                TEXT NOT NULL DEFAULT '',
    action_stale          INTEGER NOT NULL DEFAULT 0,
    phase                 TEXT CHECK (phase IS NULL OR phase IN ('planning','editing','testing','debugging','investigating')),
    active_tool           TEXT,
    active_target         TEXT,
    question              TEXT,
    last_thinking_excerpt TEXT,
    last_error            TEXT,
    tokens_input          INTEGER NOT NULL DEFAULT 0,
    tokens_output         INTEGER NOT NULL DEFAULT 0,
    tokens_cache_read     INTEGER NOT NULL DEFAULT 0,
    tokens_cache_write    INTEGER NOT NULL DEFAULT 0,
    tokens_reasoning      INTEGER NOT NULL DEFAULT 0,
    tokens_total          INTEGER NOT NULL DEFAULT 0,
    cost_usd              REAL,
    subagent_count        INTEGER NOT NULL DEFAULT 0,
    first_started_at      INTEGER NOT NULL,
    updated_at            INTEGER NOT NULL,
    UNIQUE (environment_id, harness, session_id)
);

CREATE INDEX IF NOT EXISTS idx_agents_project     ON agents(project_id);
CREATE INDEX IF NOT EXISTS idx_agents_environment ON agents(environment_id);
CREATE INDEX IF NOT EXISTS idx_agents_activity    ON agents(activity);
CREATE INDEX IF NOT EXISTS idx_agents_updated     ON agents(updated_at);
CREATE INDEX IF NOT EXISTS idx_runs_agent         ON runs(agent_id);

CREATE TABLE IF NOT EXISTS telemetry_events (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id      TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    subagent_id   TEXT,
    event_type    TEXT NOT NULL,
    timestamp     INTEGER NOT NULL,
    data_json     TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_events_agent_ts ON telemetry_events(agent_id, timestamp);
CREATE INDEX IF NOT EXISTS idx_events_subagent ON telemetry_events(agent_id, subagent_id);
CREATE INDEX IF NOT EXISTS idx_events_ts       ON telemetry_events(timestamp);

CREATE TABLE IF NOT EXISTS observer_cursors (
    environment_id   TEXT NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    last_seq         INTEGER NOT NULL,
    PRIMARY KEY (environment_id)
);
