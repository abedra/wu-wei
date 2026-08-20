use std::collections::HashMap;

use rusqlite::{Connection, params};

use super::error::DbResult;

pub fn get_all(conn: &Connection) -> DbResult<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut map = HashMap::new();
    for row in rows {
        let (key, value): (String, String) = row?;
        map.insert(key, value);
    }
    Ok(map)
}

pub fn set(conn: &Connection, key: &str, value: &str) -> DbResult<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Persists several key/value pairs at once — used to save the whole
/// Settings screen's LLM section in one call.
pub fn set_many(conn: &Connection, pairs: &[(&str, &str)]) -> DbResult<()> {
    for (key, value) in pairs {
        set(conn, key, value)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn set_then_get_all_round_trips() {
        let conn = db::open_in_memory().unwrap();
        set(&conn, "llm_provider", "anthropic").unwrap();
        set(&conn, "llm_model", "claude-opus-5").unwrap();

        let all = get_all(&conn).unwrap();
        assert_eq!(
            all.get("llm_provider").map(String::as_str),
            Some("anthropic")
        );
        assert_eq!(
            all.get("llm_model").map(String::as_str),
            Some("claude-opus-5")
        );
    }

    #[test]
    fn set_overwrites_an_existing_key() {
        let conn = db::open_in_memory().unwrap();
        set(&conn, "llm_provider", "openai").unwrap();
        set(&conn, "llm_provider", "anthropic").unwrap();

        let all = get_all(&conn).unwrap();
        assert_eq!(
            all.get("llm_provider").map(String::as_str),
            Some("anthropic")
        );
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn set_many_persists_every_pair() {
        let conn = db::open_in_memory().unwrap();
        set_many(
            &conn,
            &[("llm_provider", "openai"), ("llm_model", "gpt-4o-mini")],
        )
        .unwrap();

        let all = get_all(&conn).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(
            all.get("llm_model").map(String::as_str),
            Some("gpt-4o-mini")
        );
    }
}
