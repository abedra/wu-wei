pub mod error;
pub mod project_repo;
pub mod task_repo;

use rusqlite::Connection;
use std::path::Path;

use error::DbResult;

const SCHEMA_SQL: &str = include_str!("schema.sql");

pub fn open(path: impl AsRef<Path>) -> DbResult<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", true)?;
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
    let has_column = |name: &str| -> DbResult<bool> {
        let mut stmt = conn.prepare("SELECT 1 FROM pragma_table_info('tasks') WHERE name = ?1")?;
        Ok(stmt.exists([name])?)
    };
    if !has_column("recurrence_interval")? {
        conn.execute(
            "ALTER TABLE tasks ADD COLUMN recurrence_interval INTEGER",
            [],
        )?;
    }
    if !has_column("recurrence_unit")? {
        conn.execute("ALTER TABLE tasks ADD COLUMN recurrence_unit TEXT", [])?;
    }
    Ok(())
}
