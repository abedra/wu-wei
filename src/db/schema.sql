PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS projects (
    id          TEXT PRIMARY KEY NOT NULL,
    name        TEXT NOT NULL,
    notes       TEXT NOT NULL DEFAULT '',
    status      TEXT NOT NULL DEFAULT 'active',
    kind        TEXT NOT NULL DEFAULT 'parallel',
    created_at  TEXT NOT NULL,
    updated_at  TEXT        -- stamped by project_repo::create/update; used by sync to break edit conflicts
);

CREATE TABLE IF NOT EXISTS tasks (
    id                  TEXT PRIMARY KEY NOT NULL,
    title               TEXT NOT NULL,
    notes               TEXT NOT NULL DEFAULT '',
    project_id          TEXT REFERENCES projects(id) ON DELETE SET NULL,
    due_date            TEXT,
    defer_date          TEXT,
    completed           INTEGER NOT NULL DEFAULT 0,
    completed_at        TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT,       -- stamped by task_repo::create/update; used by sync to break edit conflicts
    estimated_minutes   INTEGER,
    recurrence_interval INTEGER,
    recurrence_unit     TEXT        -- 'days' | 'weeks' | 'months'; NULL = not recurring
);

CREATE INDEX IF NOT EXISTS idx_tasks_project_id ON tasks(project_id);
CREATE INDEX IF NOT EXISTS idx_tasks_due_date    ON tasks(due_date);
CREATE INDEX IF NOT EXISTS idx_tasks_completed   ON tasks(completed);

-- Settings edited from the in-app Settings screen (see ui::settings), e.g.
-- "llm_provider"/"llm_api_key"/"llm_base_url"/"llm_model". A value saved
-- here takes priority over its corresponding WU_WEI_* environment variable
-- (see LlmConfig::resolve) — the environment remains a first-run default,
-- not the source of truth, once something's been saved. The database file
-- location itself can't live here (it has to be known before this database
-- is open); that's remembered separately, see `db_bootstrap`.
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

-- Sync's own bookkeeping (see `sync.rs`/`db::sync_repo`): local delete
-- calls (`task_repo`/`project_repo`'s `delete`/`delete_completed`) are and
-- remain hard deletes — this just additionally records that one happened,
-- so a sync run has something to tell other devices about. `kind` is
-- "task" or "project".
CREATE TABLE IF NOT EXISTS sync_tombstones (
    kind       TEXT NOT NULL,
    id         TEXT NOT NULL,
    deleted_at TEXT NOT NULL,
    PRIMARY KEY (kind, id)
);
