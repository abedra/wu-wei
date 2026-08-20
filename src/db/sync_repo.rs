use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use super::error::DbResult;

/// A recorded deletion — sync's own bookkeeping. `kind` is `"task"` or
/// `"project"`. See `sync.rs` for how these get merged across devices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tombstone {
    pub kind: String,
    pub id: String,
    pub deleted_at: DateTime<Utc>,
}

/// Records that `id` (a `kind` of `"task"` or `"project"`) was deleted at
/// `deleted_at` — called alongside the real (hard) delete in
/// `task_repo`/`project_repo` (passing `Utc::now()`), and by `sync::merge`
/// when applying a tombstone that came from another device (passing that
/// tombstone's *original* timestamp — never "now" there, or a deletion's
/// recorded time would drift forward on every device it propagates
/// through). Idempotent: recording the same id twice just updates the
/// timestamp, since `(kind, id)` is the primary key.
pub fn record_tombstone(
    conn: &Connection,
    kind: &str,
    id: &str,
    deleted_at: DateTime<Utc>,
) -> DbResult<()> {
    conn.execute(
        "INSERT INTO sync_tombstones (kind, id, deleted_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(kind, id) DO UPDATE SET deleted_at = excluded.deleted_at",
        params![kind, id, deleted_at],
    )?;
    Ok(())
}

pub fn list_tombstones(conn: &Connection) -> DbResult<Vec<Tombstone>> {
    let mut stmt = conn.prepare("SELECT kind, id, deleted_at FROM sync_tombstones")?;
    let tombstones = stmt
        .query_map([], |row| {
            Ok(Tombstone {
                kind: row.get(0)?,
                id: row.get(1)?,
                deleted_at: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tombstones)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn record_then_list_round_trips() {
        let conn = db::open_in_memory().unwrap();
        record_tombstone(&conn, "task", "abc", Utc::now()).unwrap();
        record_tombstone(&conn, "project", "xyz", Utc::now()).unwrap();

        let tombstones = list_tombstones(&conn).unwrap();
        assert_eq!(tombstones.len(), 2);
        assert!(tombstones.iter().any(|t| t.kind == "task" && t.id == "abc"));
        assert!(
            tombstones
                .iter()
                .any(|t| t.kind == "project" && t.id == "xyz")
        );
    }

    #[test]
    fn recording_the_same_id_twice_updates_rather_than_duplicates() {
        let conn = db::open_in_memory().unwrap();
        record_tombstone(&conn, "task", "abc", Utc::now()).unwrap();
        record_tombstone(&conn, "task", "abc", Utc::now()).unwrap();

        let tombstones = list_tombstones(&conn).unwrap();
        assert_eq!(tombstones.len(), 1);
    }
}
