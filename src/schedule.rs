//! Auto-ordering for the Today view: calendar events stay in their own
//! chronological order, and tasks are slotted into the gaps between them
//! according to their time estimate (see [`plan_today`]).
//!
//! The rule, in order of precedence:
//!  * All-day events sort to the very top — they don't block any time slot.
//!  * Timed events keep their chronological order and are never moved.
//!  * A task with an estimate is placed in the earliest gap (starting from
//!    "now") where it fits end-to-end. Among estimated tasks competing for
//!    the same gap, lower-numbered `priority` goes first (unset priority
//!    counts as lowest); a task that's too long for the current gap is left
//!    for a later one, while lower-priority (or same-priority, later) tasks
//!    behind it are still free to fill the space it couldn't use.
//!  * Once the last event is past, remaining estimated tasks just queue up
//!    one after another from that point on, in that same priority order.
//!  * A task with no estimate has nothing to schedule against, so it drops
//!    to the bottom of the list, still ordered by priority among itself.
//!  * A timed event drops off the list entirely once its end time has
//!    passed — see [`is_current`]. An all-day event never expires this way.

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

/// A task's sort key for priority ordering: lower is higher priority, and an
/// unset priority sorts as if it were lowest of all (`i64::MAX`) so
/// prioritized tasks always take a fitting slot ahead of unprioritized ones.
fn priority_key(task: &Task) -> i64 {
    task.priority.unwrap_or(i64::MAX)
}

/// Whether `event` still belongs in the Today view at `now`: a timed event
/// stays until its end time passes, then drops off entirely — a meeting
/// that's already over isn't useful context for planning the rest of the
/// day. An all-day event has no meaningful "end" for this purpose (see
/// `CalendarEvent::all_day`), so it never expires this way. Exposed so
/// `ui::task_list`'s pre-schedule "Today's Events" fallback can apply the
/// same rule before `plan_today` has anything to compute against.
pub fn is_current(event: &CalendarEvent, now: DateTime<Local>) -> bool {
    event.all_day || event.end.with_timezone(&Local) > now
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

    // Timed events not yet over, in chronological order, keeping their
    // original indices — one that's already ended is dropped entirely
    // (see `is_current`), not just left in place.
    let mut timed: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.all_day && is_current(e, now))
        .map(|(i, _)| i)
        .collect();
    timed.sort_by_key(|&i| events[i].start);

    // Estimated tasks waiting for a slot, lower-numbered `priority` first
    // (unset priority last), ties broken by the caller's original order —
    // `sort_by_key` is stable. A task stays here until a gap it fits in
    // comes along.
    let mut pending: Vec<usize> = tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| estimate_minutes(t).is_some())
        .map(|(i, _)| i)
        .collect();
    pending.sort_by_key(|&i| priority_key(&tasks[i]));

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

    // Unestimated tasks sink to the bottom, by priority (ties keeping the
    // caller's original order — see `pending` above).
    let mut unestimated: Vec<usize> = tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| estimate_minutes(t).is_none())
        .map(|(i, _)| i)
        .collect();
    unestimated.sort_by_key(|&i| priority_key(&tasks[i]));
    for index in unestimated {
        rows.push(ScheduleRow::Task { index, start: None });
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

    fn prioritized_task(title: &str, estimate: Option<i64>, priority: i64) -> Task {
        let mut t = task(title, estimate);
        t.priority = Some(priority);
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

    #[test]
    fn a_lower_priority_number_task_takes_a_fitting_slot_over_a_higher_one() {
        // Both fit the same open time before the meeting; the one with the
        // lower (more urgent) priority number should be slotted first,
        // regardless of which order they were passed in.
        let events = vec![event("Meeting", (10, 0), (10, 30))];
        let tasks = vec![
            prioritized_task("Nice to have", Some(30), 5),
            prioritized_task("Urgent", Some(30), 1),
        ];
        let rows = plan_today(&events, &tasks, now_at(9, 0));

        assert_eq!(
            titles(&rows, &events, &tasks),
            ["Urgent", "Nice to have", "Meeting"]
        );
    }

    #[test]
    fn an_unset_priority_loses_a_fitting_slot_to_any_prioritized_task() {
        // Only one 30-minute task fits the gap before the meeting; the
        // prioritized one takes it even though it was listed second, and
        // the unprioritized one is bumped to after the meeting.
        let events = vec![event("Meeting", (9, 30), (10, 0))];
        let tasks = vec![
            task("No priority", Some(30)),
            prioritized_task("Prioritized", Some(30), 9),
        ];
        let rows = plan_today(&events, &tasks, now_at(9, 0));

        assert_eq!(
            titles(&rows, &events, &tasks),
            ["Prioritized", "Meeting", "No priority"]
        );
    }

    #[test]
    fn equal_priority_falls_back_to_the_caller_supplied_order() {
        let tasks = vec![
            prioritized_task("First", Some(15), 2),
            prioritized_task("Second", Some(15), 2),
        ];
        let rows = plan_today(&[], &tasks, now_at(9, 0));
        assert_eq!(titles(&rows, &[], &tasks), ["First", "Second"]);
    }

    #[test]
    fn a_higher_priority_task_that_does_not_fit_still_lets_a_lower_priority_one_through() {
        // Mirrors `a_shorter_task_fills_a_gap_a_longer_one_could_not_use`,
        // but with the roles reversed by priority instead of list order: the
        // long, high-priority (low-number) task can't fit the 20-minute gap,
        // so the short, lower-priority task behind it still gets to use it.
        let events = vec![event("Meeting", (9, 20), (10, 0))];
        let tasks = vec![
            prioritized_task("Long but urgent", Some(60), 1),
            prioritized_task("Quick, less urgent", Some(15), 5),
        ];
        let rows = plan_today(&events, &tasks, now_at(9, 0));

        assert_eq!(
            titles(&rows, &events, &tasks),
            ["Quick, less urgent", "Meeting", "Long but urgent"]
        );
    }

    #[test]
    fn unestimated_tasks_at_the_bottom_are_still_ordered_by_priority() {
        let tasks = vec![
            task("No priority", None),
            prioritized_task("Prioritized", None, 1),
        ];
        let rows = plan_today(&[], &tasks, now_at(9, 0));
        assert_eq!(titles(&rows, &[], &tasks), ["Prioritized", "No priority"]);
    }

    #[test]
    fn a_timed_event_disappears_once_its_end_time_passes() {
        let events = vec![
            event("Standup", (9, 0), (9, 30)),
            event("Lunch", (12, 0), (13, 0)),
        ];
        let rows = plan_today(&events, &[], now_at(10, 0));
        assert_eq!(titles(&rows, &events, &[]), ["Lunch"]);
    }

    #[test]
    fn an_in_progress_event_still_shows() {
        // `now` is inside the event's span, not past its end — it hasn't
        // expired yet.
        let events = vec![event("Long meeting", (9, 0), (11, 0))];
        let rows = plan_today(&events, &[], now_at(10, 0));
        assert_eq!(titles(&rows, &events, &[]), ["Long meeting"]);
    }

    #[test]
    fn an_all_day_event_never_expires() {
        let events = vec![all_day("Company holiday")];
        // Well past the all-day event's own (nominal) end-of-day timestamp.
        let rows = plan_today(&events, &[], now_at(23, 30));
        assert_eq!(titles(&rows, &events, &[]), ["Company holiday"]);
    }

    #[test]
    fn an_expired_event_no_longer_blocks_a_task_from_its_old_slot() {
        // Once "Standup" is gone, a task that couldn't have fit around it
        // is free to use that time.
        let events = vec![event("Standup", (9, 0), (9, 30))];
        let tasks = vec![task("Catch up", Some(45))];
        let rows = plan_today(&events, &tasks, now_at(9, 30));
        assert_eq!(titles(&rows, &events, &tasks), ["Catch up"]);
        assert_eq!(
            task_start(&rows, &tasks, "Catch up"),
            Some(NaiveTime::from_hms_opt(9, 30, 0).unwrap())
        );
    }
}
