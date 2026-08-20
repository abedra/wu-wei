use chrono::{DateTime, Months, NaiveDate, Utc};
use uuid::Uuid;

use crate::domain::project::ProjectId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub Uuid);

impl TaskId {
    pub fn new() -> Self {
        TaskId(Uuid::new_v4())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecurrenceUnit {
    Days,
    Weeks,
    Months,
}

impl RecurrenceUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            RecurrenceUnit::Days => "days",
            RecurrenceUnit::Weeks => "weeks",
            RecurrenceUnit::Months => "months",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "days" => Some(RecurrenceUnit::Days),
            "weeks" => Some(RecurrenceUnit::Weeks),
            "months" => Some(RecurrenceUnit::Months),
            _ => None,
        }
    }

    pub const ALL: [RecurrenceUnit; 3] = [
        RecurrenceUnit::Days,
        RecurrenceUnit::Weeks,
        RecurrenceUnit::Months,
    ];
}

/// A "repeat after completion" recurrence: the next due date is measured
/// from when the task was actually completed, not its original due date, so
/// a task due Monday but finished Thursday reschedules from Thursday.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recurrence {
    pub interval: u32,
    pub unit: RecurrenceUnit,
}

impl Recurrence {
    pub fn next_due_date(&self, completed_on: NaiveDate) -> NaiveDate {
        match self.unit {
            RecurrenceUnit::Days => completed_on + chrono::Duration::days(self.interval as i64),
            RecurrenceUnit::Weeks => completed_on + chrono::Duration::weeks(self.interval as i64),
            // Clamps to the end of the target month if the day doesn't exist
            // there (e.g. Jan 31 + 1 month -> Feb 28/29).
            RecurrenceUnit::Months => completed_on
                .checked_add_months(Months::new(self.interval))
                .unwrap_or(completed_on),
        }
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
    pub completed: bool,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// Stamped by `task_repo::create`/`update` on every write, ignoring
    /// whatever's set here — callers never manage it by hand. Used by
    /// `sync` to pick the newer side of a conflicting edit.
    pub updated_at: DateTime<Utc>,
    pub estimated_minutes: Option<i64>,
    pub recurrence: Option<Recurrence>,
}

impl Task {
    pub fn new_inbox(title: impl Into<String>) -> Self {
        let now = Utc::now();
        Task {
            id: TaskId::new(),
            title: title.into(),
            notes: String::new(),
            project_id: None,
            due_date: None,
            defer_date: None,
            completed: false,
            completed_at: None,
            created_at: now,
            updated_at: now,
            estimated_minutes: None,
            recurrence: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_recurrence_advances_from_completion_date() {
        let r = Recurrence {
            interval: 3,
            unit: RecurrenceUnit::Days,
        };
        let completed = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        assert_eq!(
            r.next_due_date(completed),
            NaiveDate::from_ymd_opt(2026, 1, 4).unwrap()
        );
    }

    #[test]
    fn weeks_recurrence_advances_by_seven_days_per_week() {
        let r = Recurrence {
            interval: 2,
            unit: RecurrenceUnit::Weeks,
        };
        let completed = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        assert_eq!(
            r.next_due_date(completed),
            NaiveDate::from_ymd_opt(2026, 1, 15).unwrap()
        );
    }

    #[test]
    fn months_recurrence_clamps_to_shorter_month_end() {
        let r = Recurrence {
            interval: 1,
            unit: RecurrenceUnit::Months,
        };
        let completed = NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
        assert_eq!(
            r.next_due_date(completed),
            NaiveDate::from_ymd_opt(2026, 2, 28).unwrap()
        );
    }

    #[test]
    fn recurrence_unit_round_trips_through_its_string_form() {
        for unit in RecurrenceUnit::ALL {
            assert_eq!(RecurrenceUnit::parse(unit.as_str()), Some(unit));
        }
        assert_eq!(RecurrenceUnit::parse("fortnights"), None);
    }
}
