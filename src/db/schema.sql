PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS projects (
    id          TEXT PRIMARY KEY NOT NULL,
    name        TEXT NOT NULL,
    notes       TEXT NOT NULL DEFAULT '',
    status      TEXT NOT NULL DEFAULT 'active',
    kind        TEXT NOT NULL DEFAULT 'parallel',
    created_at  TEXT NOT NULL
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
    estimated_minutes   INTEGER,
    recurrence_interval INTEGER,
    recurrence_unit     TEXT        -- 'days' | 'weeks' | 'months'; NULL = not recurring
);

CREATE INDEX IF NOT EXISTS idx_tasks_project_id ON tasks(project_id);
CREATE INDEX IF NOT EXISTS idx_tasks_due_date    ON tasks(due_date);
CREATE INDEX IF NOT EXISTS idx_tasks_completed   ON tasks(completed);
