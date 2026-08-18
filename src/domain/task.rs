use chrono::{DateTime, NaiveDate, Utc};
use uuid::Uuid;

use crate::domain::project::ProjectId;
use crate::domain::tag::TagId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub Uuid);

impl TaskId {
    pub fn new() -> Self {
        TaskId(Uuid::new_v4())
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub notes: String,
    pub project_id: Option<ProjectId>,
    pub due_date: Option<NaiveDate>,
    pub defer_date: Option<NaiveDate>,
    pub flagged: bool,
    pub completed: bool,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub estimated_minutes: Option<i64>,
    pub tags: Vec<TagId>,
}

impl Task {
    pub fn new_inbox(title: impl Into<String>) -> Self {
        Task {
            id: TaskId::new(),
            title: title.into(),
            notes: String::new(),
            project_id: None,
            due_date: None,
            defer_date: None,
            flagged: false,
            completed: false,
            completed_at: None,
            created_at: Utc::now(),
            estimated_minutes: None,
            tags: Vec::new(),
        }
    }
}
