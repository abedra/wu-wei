use chrono::{DateTime, Datelike, Months, NaiveDate, Utc, Weekday};
use uuid::Uuid;

use crate::domain::project::ProjectId;

/// Monday-first list of every weekday, for iterating a [`WeekdaySet`].
pub const ALL_WEEKDAYS: [Weekday; 7] = [
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
    Weekday::Sat,
    Weekday::Sun,
];

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

/// The set of weekdays a recurring task is allowed to land on, as a
/// Monday-first bitmask (bit 0 = Mon … bit 6 = Sun). A [`Recurrence`] with
/// `weekdays: None` has no day restriction; `Some(set)` rolls the computed
/// next due date forward to the nearest day that's in the set — this is what
/// makes "every weekday" skip Saturday and Sunday.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeekdaySet(u8);

impl WeekdaySet {
    const FULL_MASK: u8 = 0b0111_1111;

    /// Monday through Friday — the "weekdays only" preset.
    pub const WEEKDAYS: WeekdaySet = WeekdaySet(0b0001_1111);

    pub fn from_mask(mask: u8) -> Self {
        WeekdaySet(mask & Self::FULL_MASK)
    }

    pub fn to_mask(self) -> u8 {
        self.0
    }

    pub fn from_days(days: impl IntoIterator<Item = Weekday>) -> Self {
        let mut mask = 0u8;
        for day in days {
            mask |= 1 << day.num_days_from_monday();
        }
        WeekdaySet(mask)
    }

    pub fn contains(self, day: Weekday) -> bool {
        self.0 & (1 << day.num_days_from_monday()) != 0
    }

    /// A copy of this set with `day` added or removed.
    pub fn with(self, day: Weekday, present: bool) -> Self {
        let bit = 1 << day.num_days_from_monday();
        WeekdaySet(if present { self.0 | bit } else { self.0 & !bit })
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether every weekday is allowed — equivalent to no restriction at
    /// all, so callers normalize this back to `None` before persisting.
    pub fn is_all(self) -> bool {
        self.0 == Self::FULL_MASK
    }

    pub fn days(self) -> impl Iterator<Item = Weekday> {
        ALL_WEEKDAYS.into_iter().filter(move |d| self.contains(*d))
    }
}

/// A "repeat after completion" recurrence: the next due date is measured
/// from when the task was actually completed, not its original due date, so
/// a task due Monday but finished Thursday reschedules from Thursday.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recurrence {
    pub interval: u32,
    pub unit: RecurrenceUnit,
    /// Weekdays the repeat is allowed to fall on. `None` = any day; `Some`
    /// pushes the next occurrence forward off a disallowed day (e.g. a
    /// weekend). Normalized so a full or empty set is never stored — see
    /// [`Recurrence::with_weekdays`].
    pub weekdays: Option<WeekdaySet>,
}

impl Recurrence {
    /// A plain interval/unit recurrence with no weekday restriction.
    pub fn every(interval: u32, unit: RecurrenceUnit) -> Self {
        Recurrence {
            interval,
            unit,
            weekdays: None,
        }
    }

    /// Attaches a weekday restriction, normalizing "all seven days" and the
    /// empty set (which would mean the task could never recur) back to
    /// `None`.
    pub fn with_weekdays(mut self, weekdays: Option<WeekdaySet>) -> Self {
        self.weekdays = weekdays.filter(|s| !s.is_empty() && !s.is_all());
        self
    }

    /// A short human phrase for the recurrence, e.g. "every day", "every 2
    /// weeks", "every weekday", "every Mon, Wed, Fri" — used for the repeat
    /// indicator's tooltip in the task list.
    pub fn describe(&self) -> String {
        let unit = match (self.unit, self.interval) {
            (RecurrenceUnit::Days, 1) => "day".to_string(),
            (RecurrenceUnit::Weeks, 1) => "week".to_string(),
            (RecurrenceUnit::Months, 1) => "month".to_string(),
            (RecurrenceUnit::Days, n) => format!("{n} days"),
            (RecurrenceUnit::Weeks, n) => format!("{n} weeks"),
            (RecurrenceUnit::Months, n) => format!("{n} months"),
        };
        match self.weekdays {
            Some(set) if set == WeekdaySet::WEEKDAYS => "every weekday".to_string(),
            Some(set) => {
                let days: Vec<&str> = set
                    .days()
                    .map(|d| match d {
                        Weekday::Mon => "Mon",
                        Weekday::Tue => "Tue",
                        Weekday::Wed => "Wed",
                        Weekday::Thu => "Thu",
                        Weekday::Fri => "Fri",
                        Weekday::Sat => "Sat",
                        Weekday::Sun => "Sun",
                    })
                    .collect();
                format!("every {unit}, on {}", days.join(", "))
            }
            None => format!("every {unit}"),
        }
    }

    /// Nudges `date` forward to the first day this recurrence's weekday
    /// restriction allows — unchanged when there's no restriction. Used both
    /// to enforce off-days on a computed next occurrence and to pick a
    /// sensible first due date for a newly-created repeating task.
    pub fn snap_to_allowed(&self, date: NaiveDate) -> NaiveDate {
        match self.weekdays {
            Some(set) if !set.is_empty() && !set.is_all() => {
                let mut date = date;
                // Terminates within 6 steps: the set has at least one day.
                while !set.contains(date.weekday()) {
                    date += chrono::Duration::days(1);
                }
                date
            }
            _ => date,
        }
    }

    pub fn next_due_date(&self, completed_on: NaiveDate) -> NaiveDate {
        let base = match self.unit {
            RecurrenceUnit::Days => completed_on + chrono::Duration::days(self.interval as i64),
            RecurrenceUnit::Weeks => completed_on + chrono::Duration::weeks(self.interval as i64),
            // Clamps to the end of the target month if the day doesn't exist
            // there (e.g. Jan 31 + 1 month -> Feb 28/29).
            RecurrenceUnit::Months => completed_on
                .checked_add_months(Months::new(self.interval))
                .unwrap_or(completed_on),
        };
        self.snap_to_allowed(base)
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
    /// Lower numbers are higher priority; ties are allowed. `None` means no
    /// priority has been set. Used to break ties among tasks competing for
    /// the same slot in the Today schedule — see `schedule::plan_today`.
    pub priority: Option<i64>,
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
            priority: None,
            recurrence: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_recurrence_advances_from_completion_date() {
        let r = Recurrence::every(3, RecurrenceUnit::Days);
        let completed = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        assert_eq!(
            r.next_due_date(completed),
            NaiveDate::from_ymd_opt(2026, 1, 4).unwrap()
        );
    }

    #[test]
    fn weeks_recurrence_advances_by_seven_days_per_week() {
        let r = Recurrence::every(2, RecurrenceUnit::Weeks);
        let completed = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        assert_eq!(
            r.next_due_date(completed),
            NaiveDate::from_ymd_opt(2026, 1, 15).unwrap()
        );
    }

    #[test]
    fn months_recurrence_clamps_to_shorter_month_end() {
        let r = Recurrence::every(1, RecurrenceUnit::Months);
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

    #[test]
    fn every_weekday_recurrence_skips_over_the_weekend() {
        let every_weekday =
            Recurrence::every(1, RecurrenceUnit::Days).with_weekdays(Some(WeekdaySet::WEEKDAYS));

        // Fri 2026-08-28 -> would be Sat, rolls to Mon 2026-08-31.
        let friday = NaiveDate::from_ymd_opt(2026, 8, 28).unwrap();
        assert_eq!(friday.weekday(), Weekday::Fri);
        assert_eq!(
            every_weekday.next_due_date(friday),
            NaiveDate::from_ymd_opt(2026, 8, 31).unwrap()
        );

        // Mid-week stays on the natural +1 day.
        let tuesday = NaiveDate::from_ymd_opt(2026, 8, 25).unwrap();
        assert_eq!(
            every_weekday.next_due_date(tuesday),
            NaiveDate::from_ymd_opt(2026, 8, 26).unwrap()
        );

        // Completing a Saturday-overdue instance lands on Monday, not Sunday.
        let saturday = NaiveDate::from_ymd_opt(2026, 8, 29).unwrap();
        assert_eq!(
            every_weekday.next_due_date(saturday),
            NaiveDate::from_ymd_opt(2026, 8, 31).unwrap()
        );
    }

    #[test]
    fn weekday_restriction_also_applies_to_weekly_recurrences() {
        // "every week, but only Mon/Wed/Fri": a Friday + 1 week = next Friday,
        // already allowed.
        let mwf = Recurrence::every(1, RecurrenceUnit::Weeks).with_weekdays(Some(
            WeekdaySet::from_days([Weekday::Mon, Weekday::Wed, Weekday::Fri]),
        ));
        let friday = NaiveDate::from_ymd_opt(2026, 8, 28).unwrap();
        assert_eq!(
            mwf.next_due_date(friday),
            NaiveDate::from_ymd_opt(2026, 9, 4).unwrap()
        );

        // A Tuesday + 1 week = Tuesday, not allowed, rolls to Wednesday.
        let tuesday = NaiveDate::from_ymd_opt(2026, 8, 25).unwrap();
        assert_eq!(
            mwf.next_due_date(tuesday),
            NaiveDate::from_ymd_opt(2026, 9, 2).unwrap()
        );
    }

    #[test]
    fn with_weekdays_normalizes_a_full_or_empty_set_to_no_restriction() {
        let base = Recurrence::every(1, RecurrenceUnit::Days);
        assert_eq!(
            base.with_weekdays(Some(WeekdaySet::from_days(ALL_WEEKDAYS)))
                .weekdays,
            None
        );
        assert_eq!(
            base.with_weekdays(Some(WeekdaySet::from_mask(0))).weekdays,
            None
        );
        assert_eq!(
            base.with_weekdays(Some(WeekdaySet::WEEKDAYS)).weekdays,
            Some(WeekdaySet::WEEKDAYS)
        );
    }

    #[test]
    fn weekday_set_toggles_and_reports_membership() {
        let set = WeekdaySet::WEEKDAYS;
        assert!(set.contains(Weekday::Mon));
        assert!(!set.contains(Weekday::Sat));
        assert!(!set.with(Weekday::Mon, false).contains(Weekday::Mon));
        assert!(set.with(Weekday::Sat, true).contains(Weekday::Sat));
        assert_eq!(WeekdaySet::from_mask(set.to_mask()), set);
    }

    #[test]
    fn describe_reads_naturally_for_the_common_cases() {
        assert_eq!(
            Recurrence::every(1, RecurrenceUnit::Days).describe(),
            "every day"
        );
        assert_eq!(
            Recurrence::every(2, RecurrenceUnit::Weeks).describe(),
            "every 2 weeks"
        );
        assert_eq!(
            Recurrence::every(1, RecurrenceUnit::Days)
                .with_weekdays(Some(WeekdaySet::WEEKDAYS))
                .describe(),
            "every weekday"
        );
        assert_eq!(
            Recurrence::every(1, RecurrenceUnit::Weeks)
                .with_weekdays(Some(WeekdaySet::from_days([Weekday::Tue, Weekday::Thu])))
                .describe(),
            "every week, on Tue, Thu"
        );
    }
}
