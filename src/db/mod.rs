pub mod error;
pub mod project_repo;
pub mod tag_repo;
pub mod task_repo;

use rusqlite::Connection;
use std::path::Path;

use error::DbResult;

const SCHEMA_SQL: &str = include_str!("schema.sql");

pub fn open(path: impl AsRef<Path>) -> DbResult<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.execute_batch(SCHEMA_SQL)?;
    Ok(conn)
}

#[cfg(test)]
pub fn open_in_memory() -> DbResult<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.execute_batch(SCHEMA_SQL)?;
    Ok(conn)
}
