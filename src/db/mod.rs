pub mod error;
pub mod project_repo;
pub mod settings_repo;
pub mod sync_repo;
pub mod task_repo;

use rusqlite::Connection;
use std::path::Path;

use error::DbResult;

const SCHEMA_SQL: &str = include_str!("schema.sql");

pub fn open(path: impl AsRef<Path>) -> DbResult<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", true)?;
    // A background sync thread opens its own connection to this same file
    // (see `sync.rs`) while the UI thread's connection is also live — both
    // on the same machine, where SQLite's locking is reliable, but a short
    // wait rather than an immediate SQLITE_BUSY if they happen to touch the
    // file at the same instant is still worth having.
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch(SCHEMA_SQL)?;
    migrate(&conn)?;
    Ok(conn)
}

#[cfg(test)]
pub fn open_in_memory() -> DbResult<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.execute_batch(SCHEMA_SQL)?;
    migrate(&conn)?;
    Ok(conn)
}

/// `CREATE TABLE IF NOT EXISTS` in `schema.sql` only shapes a brand-new
/// database — it does nothing to add newly-introduced columns to a
/// pre-existing `tasks` table on disk. This adds any that are missing, so
/// upgrading an existing database is safe. A no-op on both a fresh database
/// (schema.sql already has the column) and an already-migrated one.
fn migrate(conn: &Connection) -> DbResult<()> {
    let has_column = |table: &str, column: &str| -> DbResult<bool> {
        let mut stmt = conn.prepare(&format!(
            "SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1"
        ))?;
        Ok(stmt.exists([column])?)
    };
    if !has_column("tasks", "recurrence_interval")? {
        conn.execute(
            "ALTER TABLE tasks ADD COLUMN recurrence_interval INTEGER",
            [],
        )?;
    }
    if !has_column("tasks", "recurrence_unit")? {
        conn.execute("ALTER TABLE tasks ADD COLUMN recurrence_unit TEXT", [])?;
    }
    if !has_column("tasks", "updated_at")? {
        conn.execute("ALTER TABLE tasks ADD COLUMN updated_at TEXT", [])?;
        conn.execute(
            "UPDATE tasks SET updated_at = created_at WHERE updated_at IS NULL",
            [],
        )?;
    }
    if !has_column("projects", "updated_at")? {
        conn.execute("ALTER TABLE projects ADD COLUMN updated_at TEXT", [])?;
        conn.execute(
            "UPDATE projects SET updated_at = created_at WHERE updated_at IS NULL",
            [],
        )?;
    }
    Ok(())
}
