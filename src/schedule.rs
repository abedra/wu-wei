//! Auto-ordering for the Today view: calendar events stay in their own
//! chronological order, and tasks are slotted into the gaps between them
//! according to their time estimate (see [`plan_today`]).
//!
//! The rule, in order of precedence:
//!  * All-day events sort to the very top — they don't block any time slot.
//!  * Timed events keep their chronological order and are never moved.
//!  * A task with an estimate is placed in the earliest gap (starting from
//!    "now") where it fits end-to-end. A task that's too long for the
//!    current gap is left for a later one, while shorter tasks behind it are
//!    still free to fill the space it couldn't use.
//!  * Once the last event is past, remaining estimated tasks just queue up
//!    one after another from that point on.
//!  * A task with no estimate has nothing to schedule against, so it drops
//!    to the bottom of the list.

use chrono::{DateTime, Duration, Local, NaiveTime};

use crate::calendar::CalendarEvent;
use crate::domain::task::Task;

/// One row of the Today view's computed schedule. Indices point back into
/// the `events` / `tasks` slices handed to [`plan_today`], in the caller's
/// original order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleRow {
    /// A calendar event, at `events[index]`.
    Event { index: usize },
    /// A task, at `tasks[index]`. `start` is the wall-clock time the task
    /// was slotted at, or `None` for a task with no estimate (parked at the
    /// bottom).
    Task {
        index: usize,
        start: Option<NaiveTime>,
    },
}

/// A task's usable estimate in minutes: `None` (park at the bottom) unless
/// it's a positive value.
fn estimate_minutes(task: &Task) -> Option<i64> {
    task.estimated_minutes.filter(|&m| m > 0)
}

/// Builds the interleaved event/task ordering for the Today view. `now` is
/// the anchor the first task can start at — gaps entirely in the past are
/// skipped.
pub fn plan_today(
    events: &[CalendarEvent],
    tasks: &[Task],
    now: DateTime<Local>,
) -> Vec<ScheduleRow> {
    let mut rows = Vec::with_capacity(events.len() + tasks.len());

    // All-day events first: shown, but not treated as busy time.
    for (index, event) in events.iter().enumerate() {
        if event.all_day {
            rows.push(ScheduleRow::Event { index });
        }
    }

    // Timed events in chronological order, keeping their original indices.
    let mut timed: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.all_day)
        .map(|(i, _)| i)
        .collect();
    timed.sort_by_key(|&i| events[i].start);

    // Estimated tasks waiting for a slot, in the caller's order. A task
    // stays here until a gap it fits in comes along.
    let mut pending: Vec<usize> = tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| estimate_minutes(t).is_some())
        .map(|(i, _)| i)
        .collect();

    let mut cursor = now;

    let fill_until = |cursor: &mut DateTime<Local>,
                      pending: &mut Vec<usize>,
                      rows: &mut Vec<ScheduleRow>,
                      limit: Option<DateTime<Local>>| {
        let mut i = 0;
        while i < pending.len() {
            let task_index = pending[i];
            let minutes = estimate_minutes(&tasks[task_index]).unwrap_or(0);
            let finish = *cursor + Duration::minutes(minutes);
            let fits = limit.is_none_or(|end| finish <= end);
            if fits {
                rows.push(ScheduleRow::Task {
                    index: task_index,
                    start: Some(cursor.time()),
                });
                *cursor = finish;
                pending.remove(i);
            } else {
                // Doesn't fit this gap — leave it for a later one, but keep
                // checking the shorter tasks behind it.
                i += 1;
            }
        }
    };

    for &event_index in &timed {
        let event_start = events[event_index].start.with_timezone(&Local);
        let event_end = events[event_index].end.with_timezone(&Local);

        // Slot whatever fits into the gap before this event starts.
        if event_start > cursor {
            fill_until(&mut cursor, &mut pending, &mut rows, Some(event_start));
        }

        rows.push(ScheduleRow::Event { index: event_index });

        if event_end > cursor {
            cursor = event_end;
        }
    }

    // Past the last event: remaining estimated tasks queue up from here.
    fill_until(&mut cursor, &mut pending, &mut rows, None);

    // Unestimated tasks sink to the bottom, in the caller's order.
    for (index, task) in tasks.iter().enumerate() {
        if estimate_minutes(task).is_none() {
            rows.push(ScheduleRow::Task { index, start: None });
        }
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use uuid::Uuid;

    use crate::domain::task::{Task, TaskId};

    fn now_at(hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 8, 29, hour, minute, 0)
            .single()
            .expect("valid local time")
    }

    fn event(title: &str, start: (u32, u32), end: (u32, u32)) -> CalendarEvent {
        CalendarEvent {
            id: Uuid::new_v4().to_string(),
            title: title.to_string(),
            start: now_at(start.0, start.1).with_timezone(&chrono::Utc),
            end: now_at(end.0, end.1).with_timezone(&chrono::Utc),
            all_day: false,
            location: None,
        }
    }

    fn all_day(title: &str) -> CalendarEvent {
        CalendarEvent {
            id: Uuid::new_v4().to_string(),
            title: title.to_string(),
            start: now_at(0, 0).with_timezone(&chrono::Utc),
            end: now_at(23, 59).with_timezone(&chrono::Utc),
            all_day: true,
            location: None,
        }
    }

    fn task(title: &str, estimate: Option<i64>) -> Task {
        let mut t = Task::new_inbox(title);
        t.id = TaskId(Uuid::new_v4());
        t.estimated_minutes = estimate;
        t
    }

    fn titles(rows: &[ScheduleRow], events: &[CalendarEvent], tasks: &[Task]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                ScheduleRow::Event { index } => events[*index].title.clone(),
                ScheduleRow::Task { index, .. } => tasks[*index].title.clone(),
            })
            .collect()
    }

    fn task_start(rows: &[ScheduleRow], tasks: &[Task], title: &str) -> Option<NaiveTime> {
        rows.iter().find_map(|row| match row {
            ScheduleRow::Task { index, start } if tasks[*index].title == title => Some(*start),
            _ => None,
        })?
    }

    #[test]
    fn task_is_slotted_into_the_gap_before_an_event() {
        let events = vec![event("Standup", (10, 0), (10, 30))];
        let tasks = vec![task("Write report", Some(30))];
        let rows = plan_today(&events, &tasks, now_at(9, 0));

        assert_eq!(titles(&rows, &events, &tasks), ["Write report", "Standup"]);
        assert_eq!(
            task_start(&rows, &tasks, "Write report"),
            Some(NaiveTime::from_hms_opt(9, 0, 0).unwrap())
        );
    }

    #[test]
    fn events_keep_their_chronological_order() {
        let events = vec![
            event("Later", (14, 0), (15, 0)),
            event("Earlier", (10, 0), (11, 0)),
        ];
        let rows = plan_today(&events, &[], now_at(9, 0));
        assert_eq!(titles(&rows, &events, &[]), ["Earlier", "Later"]);
    }

    #[test]
    fn a_task_too_long_for_the_gap_waits_for_the_next_one() {
        // 30-min gap, then a 2h gap after the first meeting.
        let events = vec![
            event("Meeting A", (9, 30), (10, 0)),
            event("Meeting B", (12, 0), (13, 0)),
        ];
        let tasks = vec![task("Deep work", Some(60))];
        let rows = plan_today(&events, &tasks, now_at(9, 0));

        assert_eq!(
            titles(&rows, &events, &tasks),
            ["Meeting A", "Deep work", "Meeting B"]
        );
        assert_eq!(
            task_start(&rows, &tasks, "Deep work"),
            Some(NaiveTime::from_hms_opt(10, 0, 0).unwrap())
        );
    }

    #[test]
    fn a_shorter_task_fills_a_gap_a_longer_one_could_not_use() {
        let events = vec![event("Meeting", (9, 20), (10, 0))];
        let tasks = vec![task("Long task", Some(60)), task("Quick task", Some(15))];
        let rows = plan_today(&events, &tasks, now_at(9, 0));

        // Quick task slots into the 20-minute gap; Long task waits.
        assert_eq!(
            titles(&rows, &events, &tasks),
            ["Quick task", "Meeting", "Long task"]
        );
    }

    #[test]
    fn unestimated_tasks_go_to_the_bottom() {
        let events = vec![event("Standup", (10, 0), (10, 30))];
        let tasks = vec![task("No estimate", None), task("Has estimate", Some(15))];
        let rows = plan_today(&events, &tasks, now_at(9, 0));

        assert_eq!(
            titles(&rows, &events, &tasks),
            ["Has estimate", "Standup", "No estimate"]
        );
        assert_eq!(task_start(&rows, &tasks, "No estimate"), None);
    }

    #[test]
    fn all_day_events_sort_to_the_top_without_blocking_time() {
        let events = vec![
            event("Standup", (10, 0), (10, 30)),
            all_day("Company holiday"),
        ];
        let tasks = vec![task("Errand", Some(30))];
        let rows = plan_today(&events, &tasks, now_at(9, 0));

        assert_eq!(
            titles(&rows, &events, &tasks),
            ["Company holiday", "Errand", "Standup"]
        );
    }

    #[test]
    fn remaining_tasks_queue_after_the_last_event() {
        let events = vec![event("Standup", (9, 0), (9, 30))];
        let tasks = vec![task("First", Some(60)), task("Second", Some(30))];
        let rows = plan_today(&events, &tasks, now_at(9, 0));

        assert_eq!(
            titles(&rows, &events, &tasks),
            ["Standup", "First", "Second"]
        );
        assert_eq!(
            task_start(&rows, &tasks, "First"),
            Some(NaiveTime::from_hms_opt(9, 30, 0).unwrap())
        );
        assert_eq!(
            task_start(&rows, &tasks, "Second"),
            Some(NaiveTime::from_hms_opt(10, 30, 0).unwrap())
        );
    }

    #[test]
    fn overlapping_events_are_both_kept_and_tasks_wait_them_out() {
        let events = vec![
            event("Long meeting", (9, 0), (11, 0)),
            event("Overlapping call", (10, 0), (10, 30)),
        ];
        let tasks = vec![task("Follow up", Some(30))];
        // Only a 15-minute slot before the meeting — not enough for a
        // 30-minute task, so it waits out both overlapping events.
        let rows = plan_today(&events, &tasks, now_at(8, 45));

        assert_eq!(
            titles(&rows, &events, &tasks),
            ["Long meeting", "Overlapping call", "Follow up"]
        );
        assert_eq!(
            task_start(&rows, &tasks, "Follow up"),
            Some(NaiveTime::from_hms_opt(11, 0, 0).unwrap())
        );
    }

    #[test]
    fn no_events_just_queues_tasks_from_now() {
        let tasks = vec![task("A", Some(30)), task("B", None), task("C", Some(45))];
        let rows = plan_today(&[], &tasks, now_at(13, 0));

        assert_eq!(titles(&rows, &[], &tasks), ["A", "C", "B"]);
        assert_eq!(
            task_start(&rows, &tasks, "A"),
            Some(NaiveTime::from_hms_opt(13, 0, 0).unwrap())
        );
        assert_eq!(
            task_start(&rows, &tasks, "C"),
            Some(NaiveTime::from_hms_opt(13, 30, 0).unwrap())
        );
    }
}
