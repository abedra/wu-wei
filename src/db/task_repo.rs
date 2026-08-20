use chrono::{NaiveDate, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use super::error::{DbError, DbResult};
use crate::domain::project::ProjectId;
use crate::domain::task::{Recurrence, RecurrenceUnit, Task, TaskId};

pub fn create(conn: &Connection, task: &Task) -> DbResult<()> {
    conn.execute(
        "INSERT INTO tasks (id, title, notes, project_id, due_date, defer_date,
                             completed, completed_at, created_at, estimated_minutes,
                             recurrence_interval, recurrence_unit)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            task.id.0.to_string(),
            task.title,
            task.notes,
            task.project_id.map(|p| p.0.to_string()),
            task.due_date,
            task.defer_date,
            task.completed,
            task.completed_at,
            task.created_at,
            task.estimated_minutes,
            task.recurrence.map(|r| r.interval),
            task.recurrence.map(|r| r.unit.as_str()),
        ],
    )?;
    Ok(())
}

pub fn update(conn: &Connection, task: &Task) -> DbResult<()> {
    conn.execute(
        "UPDATE tasks SET title = ?2, notes = ?3, project_id = ?4, due_date = ?5,
                           defer_date = ?6, completed = ?7,
                           completed_at = ?8, estimated_minutes = ?9,
                           recurrence_interval = ?10, recurrence_unit = ?11
         WHERE id = ?1",
        params![
            task.id.0.to_string(),
            task.title,
            task.notes,
            task.project_id.map(|p| p.0.to_string()),
            task.due_date,
            task.defer_date,
            task.completed,
            task.completed_at,
            task.estimated_minutes,
            task.recurrence.map(|r| r.interval),
            task.recurrence.map(|r| r.unit.as_str()),
        ],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, id: TaskId) -> DbResult<()> {
    let rows = conn.execute("DELETE FROM tasks WHERE id = ?1", params![id.0.to_string()])?;
    if rows == 0 {
        return Err(DbError::NotFound(format!("task {}", id.0)));
    }
    Ok(())
}

/// Permanently deletes every completed task — the "Archive Completed"
/// action in the Completed view. Returns how many were removed.
pub fn delete_completed(conn: &Connection) -> DbResult<usize> {
    let rows = conn.execute("DELETE FROM tasks WHERE completed = 1", [])?;
    Ok(rows)
}

pub fn get(conn: &Connection, id: TaskId) -> DbResult<Option<Task>> {
    conn.query_row(
        "SELECT id, title, notes, project_id, due_date, defer_date,
                completed, completed_at, created_at, estimated_minutes,
                recurrence_interval, recurrence_unit
         FROM tasks WHERE id = ?1",
        params![id.0.to_string()],
        row_to_task,
    )
    .optional()
    .map_err(Into::into)
}

pub fn list_all(conn: &Connection) -> DbResult<Vec<Task>> {
    list_where(conn, "1 = 1", params![], "created_at")
}

pub fn list_inbox(conn: &Connection) -> DbResult<Vec<Task>> {
    list_where(
        conn,
        "project_id IS NULL AND completed = 0",
        params![],
        "created_at",
    )
}

pub fn list_by_project(conn: &Connection, project_id: ProjectId) -> DbResult<Vec<Task>> {
    list_where(
        conn,
        "project_id = ?1 AND completed = 0",
        params![project_id.0.to_string()],
        "created_at",
    )
}

pub fn list_today(conn: &Connection, today: NaiveDate) -> DbResult<Vec<Task>> {
    list_where(
        conn,
        "due_date IS NOT NULL AND due_date <= ?1 AND completed = 0",
        params![today],
        "due_date",
    )
}

/// Strictly before `today` — tasks due today itself belong in `list_today`,
/// not here.
pub fn list_overdue(conn: &Connection, today: NaiveDate) -> DbResult<Vec<Task>> {
    list_where(
        conn,
        "due_date IS NOT NULL AND due_date < ?1 AND completed = 0",
        params![today],
        "due_date",
    )
}

pub fn list_completed(conn: &Connection) -> DbResult<Vec<Task>> {
    list_where(conn, "completed = 1", params![], "completed_at DESC")
}

pub fn set_completed(conn: &Connection, id: TaskId, completed: bool) -> DbResult<()> {
    let completed_at = completed.then(Utc::now);
    conn.execute(
        "UPDATE tasks SET completed = ?2, completed_at = ?3 WHERE id = ?1",
        params![id.0.to_string(), completed, completed_at],
    )?;
    Ok(())
}

pub fn set_due_date(conn: &Connection, id: TaskId, due_date: Option<NaiveDate>) -> DbResult<()> {
    conn.execute(
        "UPDATE tasks SET due_date = ?2 WHERE id = ?1",
        params![id.0.to_string(), due_date],
    )?;
    Ok(())
}

pub fn set_project(conn: &Connection, id: TaskId, project_id: Option<ProjectId>) -> DbResult<()> {
    conn.execute(
        "UPDATE tasks SET project_id = ?2 WHERE id = ?1",
        params![id.0.to_string(), project_id.map(|p| p.0.to_string())],
    )?;
    Ok(())
}

pub fn set_recurrence(
    conn: &Connection,
    id: TaskId,
    recurrence: Option<Recurrence>,
) -> DbResult<()> {
    conn.execute(
        "UPDATE tasks SET recurrence_interval = ?2, recurrence_unit = ?3 WHERE id = ?1",
        params![
            id.0.to_string(),
            recurrence.map(|r| r.interval),
            recurrence.map(|r| r.unit.as_str()),
        ],
    )?;
    Ok(())
}

/// All not-yet-completed tasks, regardless of perspective — the working set
/// an AI chat command can reference by id (see `crate::llm::ChatContext`).
pub fn list_open(conn: &Connection) -> DbResult<Vec<Task>> {
    list_where(conn, "completed = 0", params![], "created_at")
}

fn list_where(
    conn: &Connection,
    where_clause: &str,
    query_params: impl rusqlite::Params,
    order_by: &str,
) -> DbResult<Vec<Task>> {
    let sql = format!(
        "SELECT id, title, notes, project_id, due_date, defer_date,
                completed, completed_at, created_at, estimated_minutes,
                recurrence_interval, recurrence_unit
         FROM tasks WHERE {where_clause} ORDER BY {order_by}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let tasks: Vec<Task> = stmt
        .query_map(query_params, row_to_task)?
        .collect::<Result<_, _>>()?;
    Ok(tasks)
}

fn row_to_task(row: &rusqlite::Row) -> rusqlite::Result<Task> {
    let id: String = row.get(0)?;
    let project_id: Option<String> = row.get(3)?;
    let recurrence_interval: Option<u32> = row.get(10)?;
    let recurrence_unit: Option<String> = row.get(11)?;
    let recurrence = recurrence_interval
        .zip(recurrence_unit)
        .map(|(interval, unit)| Recurrence {
            interval,
            unit: RecurrenceUnit::parse(&unit)
                .expect("recurrence_unit column always holds a value written by this module"),
        });
    Ok(Task {
        id: TaskId(
            Uuid::parse_str(&id)
                .expect("id column always holds a valid uuid written by this module"),
        ),
        title: row.get(1)?,
        notes: row.get(2)?,
        project_id: project_id.map(|s| {
            ProjectId(
                Uuid::parse_str(&s)
                    .expect("project_id column always holds a valid uuid written by this module"),
            )
        }),
        due_date: row.get(4)?,
        defer_date: row.get(5)?,
        completed: row.get(6)?,
        completed_at: row.get(7)?,
        created_at: row.get(8)?,
        estimated_minutes: row.get(9)?,
        recurrence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::domain::project::Project;
    use chrono::Duration;

    #[test]
    fn create_get_update_delete() {
        let conn = db::open_in_memory().unwrap();
        let mut task = Task::new_inbox("write tests");
        create(&conn, &task).unwrap();

        let fetched = get(&conn, task.id).unwrap().unwrap();
        assert_eq!(fetched.title, "write tests");
        assert!(fetched.project_id.is_none());
        assert!(fetched.recurrence.is_none());

        task.title = "write more tests".to_string();
        task.notes = "some notes".to_string();
        update(&conn, &task).unwrap();
        let fetched = get(&conn, task.id).unwrap().unwrap();
        assert_eq!(fetched.title, "write more tests");
        assert_eq!(fetched.notes, "some notes");

        delete(&conn, task.id).unwrap();
        assert!(get(&conn, task.id).unwrap().is_none());
    }

    #[test]
    fn set_due_date_and_set_project_update_only_that_field() {
        let conn = db::open_in_memory().unwrap();
        let project = Project::new("Errands");
        db::project_repo::create(&conn, &project).unwrap();

        let task = Task::new_inbox("call the bank");
        create(&conn, &task).unwrap();

        let due = Utc::now().date_naive();
        set_due_date(&conn, task.id, Some(due)).unwrap();
        let fetched = get(&conn, task.id).unwrap().unwrap();
        assert_eq!(fetched.due_date, Some(due));
        assert_eq!(fetched.title, "call the bank");

        set_project(&conn, task.id, Some(project.id)).unwrap();
        let fetched = get(&conn, task.id).unwrap().unwrap();
        assert_eq!(fetched.project_id, Some(project.id));
        assert_eq!(fetched.due_date, Some(due));

        set_project(&conn, task.id, None).unwrap();
        let fetched = get(&conn, task.id).unwrap().unwrap();
        assert!(fetched.project_id.is_none());
    }

    #[test]
    fn list_open_excludes_completed() {
        let conn = db::open_in_memory().unwrap();
        let open_task = Task::new_inbox("open");
        create(&conn, &open_task).unwrap();
        let mut done_task = Task::new_inbox("done");
        done_task.completed = true;
        create(&conn, &done_task).unwrap();

        let open = list_open(&conn).unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].title, "open");
    }

    #[test]
    fn delete_completed_removes_only_completed_tasks_and_reports_the_count() {
        let conn = db::open_in_memory().unwrap();
        let open_task = Task::new_inbox("open");
        create(&conn, &open_task).unwrap();
        let mut done_task_1 = Task::new_inbox("done 1");
        done_task_1.completed = true;
        create(&conn, &done_task_1).unwrap();
        let mut done_task_2 = Task::new_inbox("done 2");
        done_task_2.completed = true;
        create(&conn, &done_task_2).unwrap();

        let removed = delete_completed(&conn).unwrap();

        assert_eq!(removed, 2);
        let remaining = list_all(&conn).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].title, "open");
    }

    #[test]
    fn perspective_queries() {
        let conn = db::open_in_memory().unwrap();

        let project = Project::new("Test Project");
        db::project_repo::create(&conn, &project).unwrap();

        let inbox_task = Task::new_inbox("inbox item");
        create(&conn, &inbox_task).unwrap();

        let mut project_task = Task::new_inbox("project item");
        project_task.project_id = Some(project.id);
        create(&conn, &project_task).unwrap();

        let today = Utc::now().date_naive();
        let mut today_task = Task::new_inbox("due today");
        today_task.due_date = Some(today);
        create(&conn, &today_task).unwrap();

        let mut overdue_task = Task::new_inbox("overdue");
        overdue_task.due_date = Some(today - Duration::days(1));
        create(&conn, &overdue_task).unwrap();

        let mut future_task = Task::new_inbox("future");
        future_task.due_date = Some(today + Duration::days(5));
        create(&conn, &future_task).unwrap();

        let inbox = list_inbox(&conn).unwrap();
        assert_eq!(inbox.len(), 4); // everything except project_task
        assert!(inbox.iter().all(|t| t.project_id.is_none()));

        let by_project = list_by_project(&conn, project.id).unwrap();
        assert_eq!(by_project.len(), 1);
        assert_eq!(by_project[0].title, "project item");

        set_completed(&conn, project_task.id, true).unwrap();
        let by_project = list_by_project(&conn, project.id).unwrap();
        assert!(by_project.is_empty());

        let today_list = list_today(&conn, today).unwrap();
        let today_titles: Vec<&str> = today_list.iter().map(|t| t.title.as_str()).collect();
        assert!(today_titles.contains(&"due today"));
        assert!(today_titles.contains(&"overdue"));
        assert!(!today_titles.contains(&"future"));

        let overdue_list = list_overdue(&conn, today).unwrap();
        assert_eq!(overdue_list.len(), 1);
        assert_eq!(overdue_list[0].title, "overdue");

        set_completed(&conn, overdue_task.id, true).unwrap();
        let overdue_list = list_overdue(&conn, today).unwrap();
        assert!(overdue_list.is_empty());

        set_completed(&conn, today_task.id, true).unwrap();
        let today_list = list_today(&conn, today).unwrap();
        assert!(!today_list.iter().any(|t| t.id == today_task.id));

        let completed = list_completed(&conn).unwrap();
        assert_eq!(completed.len(), 3);
        assert!(completed.iter().all(|t| t.completed_at.is_some()));
    }

    #[test]
    fn recurrence_round_trips_through_create_get_and_update() {
        let conn = db::open_in_memory().unwrap();
        let mut task = Task::new_inbox("water plants");
        task.recurrence = Some(Recurrence {
            interval: 3,
            unit: RecurrenceUnit::Days,
        });
        create(&conn, &task).unwrap();

        let fetched = get(&conn, task.id).unwrap().unwrap();
        assert_eq!(
            fetched.recurrence,
            Some(Recurrence {
                interval: 3,
                unit: RecurrenceUnit::Days,
            })
        );

        task.recurrence = None;
        update(&conn, &task).unwrap();
        let fetched = get(&conn, task.id).unwrap().unwrap();
        assert!(fetched.recurrence.is_none());
    }
}
