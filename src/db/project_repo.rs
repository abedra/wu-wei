use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use super::error::{DbError, DbResult};
use crate::domain::project::{Project, ProjectId, ProjectKind, ProjectStatus};

pub fn create(conn: &Connection, project: &Project) -> DbResult<()> {
    conn.execute(
        "INSERT INTO projects (id, name, notes, status, kind, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            project.id.0.to_string(),
            project.name,
            project.notes,
            project.status.as_str(),
            project.kind.as_str(),
            project.created_at,
        ],
    )?;
    Ok(())
}

pub fn update(conn: &Connection, project: &Project) -> DbResult<()> {
    conn.execute(
        "UPDATE projects SET name = ?2, notes = ?3, status = ?4, kind = ?5 WHERE id = ?1",
        params![
            project.id.0.to_string(),
            project.name,
            project.notes,
            project.status.as_str(),
            project.kind.as_str(),
        ],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, id: ProjectId) -> DbResult<()> {
    let rows = conn.execute(
        "DELETE FROM projects WHERE id = ?1",
        params![id.0.to_string()],
    )?;
    if rows == 0 {
        return Err(DbError::NotFound(format!("project {}", id.0)));
    }
    Ok(())
}

pub fn get(conn: &Connection, id: ProjectId) -> DbResult<Option<Project>> {
    conn.query_row(
        "SELECT id, name, notes, status, kind, created_at FROM projects WHERE id = ?1",
        params![id.0.to_string()],
        row_to_project,
    )
    .optional()
    .map_err(DbError::from)?
    .transpose()
}

pub fn list_all(conn: &Connection) -> DbResult<Vec<Project>> {
    let mut stmt = conn
        .prepare("SELECT id, name, notes, status, kind, created_at FROM projects ORDER BY name")?;
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
            created_at: row.get(5)?,
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
    fn list_all_orders_by_name() {
        let conn = db::open_in_memory().unwrap();
        create(&conn, &Project::new("Zebra")).unwrap();
        create(&conn, &Project::new("Apple")).unwrap();
        let projects = list_all(&conn).unwrap();
        assert_eq!(projects[0].name, "Apple");
        assert_eq!(projects[1].name, "Zebra");
    }
}
