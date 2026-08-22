use rusqlite::{Connection, params};
#[cfg(test)]
use rusqlite::OptionalExtension;
use uuid::Uuid;

use super::error::{DbError, DbResult};
use super::sync_repo;
use crate::domain::project::{Project, ProjectId, ProjectKind, ProjectStatus};

/// Stamps `updated_at` to now, ignoring whatever's on `project` for that
/// column — see the field's doc comment on `domain::project::Project`.
pub fn create(conn: &Connection, project: &Project) -> DbResult<()> {
    conn.execute(
        "INSERT INTO projects (id, name, notes, status, kind, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            project.id.0.to_string(),
            project.name,
            project.notes,
            project.status.as_str(),
            project.kind.as_str(),
            project.created_at,
            chrono::Utc::now(),
        ],
    )?;
    Ok(())
}

/// Stamps `updated_at` to now — see `create`.
pub fn update(conn: &Connection, project: &Project) -> DbResult<()> {
    conn.execute(
        "UPDATE projects SET name = ?2, notes = ?3, status = ?4, kind = ?5, updated_at = ?6
         WHERE id = ?1",
        params![
            project.id.0.to_string(),
            project.name,
            project.notes,
            project.status.as_str(),
            project.kind.as_str(),
            chrono::Utc::now(),
        ],
    )?;
    Ok(())
}

/// Hard delete, unchanged — additionally records a tombstone (see
/// `sync_repo`) so a later sync run has something to tell other devices.
pub fn delete(conn: &Connection, id: ProjectId) -> DbResult<()> {
    let id_str = id.0.to_string();
    let rows = conn.execute("DELETE FROM projects WHERE id = ?1", params![id_str])?;
    if rows == 0 {
        return Err(DbError::NotFound(format!("project {}", id.0)));
    }
    sync_repo::record_tombstone(conn, "project", &id_str, chrono::Utc::now())?;
    Ok(())
}

/// Writes `project` exactly as given — including `created_at`/`updated_at`
/// verbatim, unlike `create`/`update` — inserting it if the id is new,
/// overwriting it in place if not. Used only by `sync::merge` — see
/// `task_repo::upsert_synced` for why.
pub fn upsert_synced(conn: &Connection, project: &Project) -> DbResult<()> {
    conn.execute(
        "INSERT INTO projects (id, name, notes, status, kind, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
             name = excluded.name, notes = excluded.notes,
             status = excluded.status, kind = excluded.kind,
             updated_at = excluded.updated_at",
        params![
            project.id.0.to_string(),
            project.name,
            project.notes,
            project.status.as_str(),
            project.kind.as_str(),
            project.created_at,
            project.updated_at,
        ],
    )?;
    Ok(())
}

/// Deletes `id` if it exists, silently doing nothing if it doesn't (unlike
/// `delete`, which errors), and does *not* record its own tombstone — see
/// `task_repo::delete_if_exists`. Used only by `sync::merge`.
pub fn delete_if_exists(conn: &Connection, id: ProjectId) -> DbResult<()> {
    conn.execute(
        "DELETE FROM projects WHERE id = ?1",
        params![id.0.to_string()],
    )?;
    Ok(())
}

/// Fetches a single project by id. Only the tests need this today (production
/// code loads projects via `list_all`), so it's gated to test builds to avoid
/// a dead-code warning; promote to `pub` if a caller appears.
#[cfg(test)]
pub fn get(conn: &Connection, id: ProjectId) -> DbResult<Option<Project>> {
    conn.query_row(
        "SELECT id, name, notes, status, kind, created_at, updated_at FROM projects WHERE id = ?1",
        params![id.0.to_string()],
        row_to_project,
    )
    .optional()
    .map_err(DbError::from)?
    .transpose()
}

pub fn list_all(conn: &Connection) -> DbResult<Vec<Project>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, notes, status, kind, created_at, updated_at FROM projects ORDER BY name",
    )?;
    let projects = stmt
        .query_map(params![], row_to_project)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .collect::<DbResult<Vec<_>>>()?;
    Ok(projects)
}

fn row_to_project(row: &rusqlite::Row) -> rusqlite::Result<DbResult<Project>> {
    let id: String = row.get(0)?;
    let status_str: String = row.get(3)?;
    let kind_str: String = row.get(4)?;
    let created_at: chrono::DateTime<chrono::Utc> = row.get(5)?;
    // Falls back to `created_at` for a row somehow read before `migrate`'s
    // backfill ran — see the identical fallback in `task_repo::row_to_task`.
    let updated_at: Option<chrono::DateTime<chrono::Utc>> = row.get(6)?;
    Ok((|| {
        let id = Uuid::parse_str(&id)?;
        let status = ProjectStatus::parse(&status_str)
            .ok_or_else(|| DbError::InvalidEnumValue(status_str.clone()))?;
        let kind = ProjectKind::parse(&kind_str)
            .ok_or_else(|| DbError::InvalidEnumValue(kind_str.clone()))?;
        Ok(Project {
            id: ProjectId(id),
            name: row.get(1)?,
            notes: row.get(2)?,
            status,
            kind,
            created_at,
            updated_at: updated_at.unwrap_or(created_at),
        })
    })())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn create_get_update_delete() {
        let conn = db::open_in_memory().unwrap();
        let mut project = Project::new("Kitchen Remodel");
        create(&conn, &project).unwrap();

        let fetched = get(&conn, project.id).unwrap().unwrap();
        assert_eq!(fetched.name, "Kitchen Remodel");
        assert_eq!(fetched.status, ProjectStatus::Active);
        assert_eq!(fetched.kind, ProjectKind::Parallel);

        project.name = "Kitchen Remodel v2".to_string();
        project.status = ProjectStatus::OnHold;
        project.kind = ProjectKind::Sequential;
        update(&conn, &project).unwrap();

        let fetched = get(&conn, project.id).unwrap().unwrap();
        assert_eq!(fetched.name, "Kitchen Remodel v2");
        assert_eq!(fetched.status, ProjectStatus::OnHold);
        assert_eq!(fetched.kind, ProjectKind::Sequential);

        delete(&conn, project.id).unwrap();
        assert!(get(&conn, project.id).unwrap().is_none());
    }

    #[test]
    fn create_and_update_stamp_updated_at() {
        let conn = db::open_in_memory().unwrap();
        let mut project = Project::new("Stamped");
        create(&conn, &project).unwrap();
        let after_create = get(&conn, project.id).unwrap().unwrap().updated_at;

        project.name = "Stamped again".to_string();
        update(&conn, &project).unwrap();
        let after_update = get(&conn, project.id).unwrap().unwrap().updated_at;

        assert!(after_update >= after_create);
    }

    #[test]
    fn delete_records_a_tombstone() {
        let conn = db::open_in_memory().unwrap();
        let project = Project::new("Gone soon");
        create(&conn, &project).unwrap();

        delete(&conn, project.id).unwrap();

        let tombstones = crate::db::sync_repo::list_tombstones(&conn).unwrap();
        assert_eq!(tombstones.len(), 1);
        assert_eq!(tombstones[0].kind, "project");
        assert_eq!(tombstones[0].id, project.id.0.to_string());
    }

    #[test]
    fn list_all_orders_by_name() {
        let conn = db::open_in_memory().unwrap();
        create(&conn, &Project::new("Zebra")).unwrap();
        create(&conn, &Project::new("Apple")).unwrap();
        let projects = list_all(&conn).unwrap();
        assert_eq!(projects[0].name, "Apple");
        assert_eq!(projects[1].name, "Zebra");
    }
}
