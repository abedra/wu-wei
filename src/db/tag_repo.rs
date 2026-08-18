use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use super::error::DbResult;
use crate::domain::tag::{Tag, TagId};

pub fn create(conn: &Connection, tag: &Tag) -> DbResult<()> {
    conn.execute(
        "INSERT INTO tags (id, name) VALUES (?1, ?2)",
        params![tag.id.0.to_string(), tag.name],
    )?;
    Ok(())
}

pub fn update(conn: &Connection, tag: &Tag) -> DbResult<()> {
    conn.execute(
        "UPDATE tags SET name = ?2 WHERE id = ?1",
        params![tag.id.0.to_string(), tag.name],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, id: TagId) -> DbResult<()> {
    conn.execute("DELETE FROM tags WHERE id = ?1", params![id.0.to_string()])?;
    Ok(())
}

pub fn list_all(conn: &Connection) -> DbResult<Vec<Tag>> {
    let mut stmt = conn.prepare("SELECT id, name FROM tags ORDER BY name")?;
    let tags = stmt
        .query_map(params![], row_to_tag)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tags)
}

pub fn get_by_name(conn: &Connection, name: &str) -> DbResult<Option<Tag>> {
    conn.query_row(
        "SELECT id, name FROM tags WHERE name = ?1",
        params![name],
        row_to_tag,
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_or_create_by_name(conn: &Connection, name: &str) -> DbResult<Tag> {
    if let Some(tag) = get_by_name(conn, name)? {
        return Ok(tag);
    }
    let tag = Tag {
        id: TagId::new(),
        name: name.to_string(),
    };
    create(conn, &tag)?;
    Ok(tag)
}

fn row_to_tag(row: &rusqlite::Row) -> rusqlite::Result<Tag> {
    let id: String = row.get(0)?;
    Ok(Tag {
        id: TagId(
            Uuid::parse_str(&id)
                .expect("id column always holds a valid uuid written by this module"),
        ),
        name: row.get(1)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn create_list_delete() {
        let conn = db::open_in_memory().unwrap();
        let tag = Tag {
            id: TagId::new(),
            name: "errand".to_string(),
        };
        create(&conn, &tag).unwrap();

        let tags = list_all(&conn).unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "errand");

        delete(&conn, tag.id).unwrap();
        assert!(list_all(&conn).unwrap().is_empty());
    }

    #[test]
    fn update_renames() {
        let conn = db::open_in_memory().unwrap();
        let mut tag = Tag {
            id: TagId::new(),
            name: "errand".to_string(),
        };
        create(&conn, &tag).unwrap();

        tag.name = "chore".to_string();
        update(&conn, &tag).unwrap();

        let tags = list_all(&conn).unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "chore");
    }

    #[test]
    fn get_or_create_is_idempotent() {
        let conn = db::open_in_memory().unwrap();
        let a = get_or_create_by_name(&conn, "work").unwrap();
        let b = get_or_create_by_name(&conn, "work").unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(list_all(&conn).unwrap().len(), 1);
    }
}
