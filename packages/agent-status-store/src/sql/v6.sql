
CREATE TABLE agents_new (
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
    activity              TEXT NOT NULL CHECK (activity IN ('BUSY','WAITING_QUESTION','ERROR','RATE_LIMITED','IDLE')),
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
    is_hidden             INTEGER NOT NULL DEFAULT 0,
    hidden_at             INTEGER,
    active_tool_started_at INTEGER,
    active_subagents      TEXT,
    model                 TEXT,
    context_window        INTEGER,
    context_fullness      REAL,
    UNIQUE (environment_id, harness, session_id)
);

INSERT INTO agents_new (
    id, session_id, environment_id, harness,
    project_id, project_name, canonical_root,
    branch, is_detached, is_worktree, worktree_rel_path, cwd,
    current_run_id, activity, title, action, action_stale,
    phase, active_tool, active_target, active_tool_started_at, question,
    last_thinking_excerpt, last_error,
    tokens_input, tokens_output, tokens_cache_read, tokens_cache_write,
    tokens_reasoning, tokens_total,
    cost_usd, subagent_count, active_subagents,
    first_started_at, updated_at,
    is_hidden, hidden_at,
    model, context_window, context_fullness
)
SELECT
    id, session_id, environment_id, harness,
    project_id, project_name, canonical_root,
    branch, is_detached, is_worktree, worktree_rel_path, cwd,
    current_run_id, activity, title, action, action_stale,
    phase, active_tool, active_target, active_tool_started_at, question,
    last_thinking_excerpt, last_error,
    tokens_input, tokens_output, tokens_cache_read, tokens_cache_write,
    tokens_reasoning, tokens_total,
    cost_usd, subagent_count, active_subagents,
    first_started_at, updated_at,
    is_hidden, hidden_at,
    model, context_window, context_fullness
FROM agents;

DROP TABLE agents;
ALTER TABLE agents_new RENAME TO agents;

CREATE INDEX IF NOT EXISTS idx_agents_project     ON agents(project_id);
CREATE INDEX IF NOT EXISTS idx_agents_environment ON agents(environment_id);
CREATE INDEX IF NOT EXISTS idx_agents_activity    ON agents(activity);
CREATE INDEX IF NOT EXISTS idx_agents_updated     ON agents(updated_at);
CREATE INDEX IF NOT EXISTS idx_agents_hidden      ON agents(is_hidden);
