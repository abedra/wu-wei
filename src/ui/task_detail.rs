use chrono::{Datelike, NaiveDate, Weekday};
use eframe::egui;
use egui_extras::DatePickerButton;

use crate::domain::task::{ALL_WEEKDAYS, Recurrence, RecurrenceUnit, WeekdaySet};
use crate::state::AppState;

fn weekday_abbrev(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "Mo",
        Weekday::Tue => "Tu",
        Weekday::Wed => "We",
        Weekday::Thu => "Th",
        Weekday::Fri => "Fr",
        Weekday::Sat => "Sa",
        Weekday::Sun => "Su",
    }
}

/// The "only on these weekdays" row, shown under "Repeats every". A repeat
/// with every day selected is the same as no restriction, so that state is
/// stored as `weekdays: None` (via `Recurrence::with_weekdays`). Clicking
/// the last remaining day back off is ignored — a repeat has to be allowed
/// to land somewhere.
fn recurrence_weekday_row(ui: &mut egui::Ui, r: &mut Recurrence) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("On");
        let current = r
            .weekdays
            .unwrap_or_else(|| WeekdaySet::from_days(ALL_WEEKDAYS));
        for day in ALL_WEEKDAYS {
            let selected = current.contains(day);
            if ui
                .selectable_label(selected, weekday_abbrev(day))
                .on_hover_text(day.to_string())
                .clicked()
            {
                let next = current.with(day, !selected);
                if !next.is_empty() {
                    *r = r.with_weekdays(Some(next));
                    changed = true;
                }
            }
        }
        if ui
            .small_button("Weekdays")
            .on_hover_text("Monday–Friday only")
            .clicked()
        {
            *r = r.with_weekdays(Some(WeekdaySet::WEEKDAYS));
            changed = true;
        }
    });
    changed
}

fn naive_to_jiff(d: NaiveDate) -> jiff::civil::Date {
    jiff::civil::Date::new(d.year() as i16, d.month() as i8, d.day() as i8)
        .expect("chrono NaiveDate is always in jiff's representable range")
}

fn jiff_to_naive(d: jiff::civil::Date) -> NaiveDate {
    NaiveDate::from_ymd_opt(d.year() as i32, d.month() as u32, d.day() as u32)
        .expect("jiff Date is always a valid calendar date")
}

/// A recurrence with no due date never shows up anywhere (Today, the list's
/// Due column, ...), so enabling "Repeats every" also sets one — today — if
/// the task doesn't already have one, rather than leaving it invisible until
/// edited by hand.
fn recurrence_field(
    ui: &mut egui::Ui,
    recurrence: &mut Option<Recurrence>,
    due_date: &mut Option<NaiveDate>,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut repeats = recurrence.is_some();
        if ui.checkbox(&mut repeats, "Repeats every").changed() {
            *recurrence = if repeats {
                if due_date.is_none() {
                    *due_date = Some(chrono::Local::now().date_naive());
                }
                Some(Recurrence::every(1, RecurrenceUnit::Weeks))
            } else {
                None
            };
            changed = true;
        }
        if let Some(r) = recurrence {
            let mut interval = r.interval;
            if ui
                .add(egui::DragValue::new(&mut interval).range(1..=999))
                .changed()
            {
                r.interval = interval.max(1);
                changed = true;
            }
            let label = match r.unit {
                RecurrenceUnit::Days => "day(s)",
                RecurrenceUnit::Weeks => "week(s)",
                RecurrenceUnit::Months => "month(s)",
            };
            egui::ComboBox::from_id_salt("recurrence_unit")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    for unit in RecurrenceUnit::ALL {
                        let text = match unit {
                            RecurrenceUnit::Days => "day(s)",
                            RecurrenceUnit::Weeks => "week(s)",
                            RecurrenceUnit::Months => "month(s)",
                        };
                        if ui.selectable_label(r.unit == unit, text).clicked() {
                            r.unit = unit;
                            changed = true;
                        }
                    }
                });
        }
    });
    if let Some(r) = recurrence {
        changed |= recurrence_weekday_row(ui, r);
    }
    changed
}

/// Optional time estimate, stored as whole minutes (`estimated_minutes`).
/// Same checkbox-to-enable shape as [`date_field`]: unchecking clears it,
/// checking seeds a default of 15 minutes.
fn estimate_field(ui: &mut egui::Ui, minutes: &mut Option<i64>) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut has_estimate = minutes.is_some();
        if ui.checkbox(&mut has_estimate, "Estimate").changed() {
            *minutes = has_estimate.then_some(15);
            changed = true;
        }
        if let Some(m) = minutes {
            let mut value = (*m).max(0);
            if ui
                .add(
                    egui::DragValue::new(&mut value)
                        .range(0..=100_000)
                        .suffix(" min"),
                )
                .changed()
            {
                *m = value.max(0);
                changed = true;
            }
            if *m >= 60 {
                ui.weak(crate::ui::format_estimate(*m));
            }
        }
    });
    changed
}

fn date_field(
    ui: &mut egui::Ui,
    label: &str,
    date: &mut Option<NaiveDate>,
    show_today_button: bool,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut has_date = date.is_some();
        if ui.checkbox(&mut has_date, label).changed() {
            *date = if has_date {
                Some(chrono::Local::now().date_naive())
            } else {
                None
            };
            changed = true;
        }
        if let Some(d) = date {
            let mut jiff_date = naive_to_jiff(*d);
            if ui.add(DatePickerButton::new(&mut jiff_date)).changed() {
                *d = jiff_to_naive(jiff_date);
                changed = true;
            }
            // A one-click way back to today after the calendar's been
            // navigated elsewhere — the calendar itself has no shortcut for
            // "today" beyond paging back to the current month by hand.
            if show_today_button && ui.small_button("Today").clicked() {
                let today = chrono::Local::now().date_naive();
                if *d != today {
                    *d = today;
                    changed = true;
                }
            }
        }
    });
    changed
}

pub fn draw(ui: &mut egui::Ui, state: &mut AppState) {
    if state.task_edit_buffer.is_none() {
        ui.label("Select a task.");
        return;
    }

    // Cloned so the project ComboBox doesn't need to borrow `state` while
    // `buf` (borrowed from state.task_edit_buffer) is alive below.
    let projects = state.projects.clone();

    let mut dirty = false;
    let mut toggle_completed: Option<bool> = None;
    let mut delete_clicked = false;

    let task_id = {
        let buf = state.task_edit_buffer.as_mut().unwrap();
        let task_id = buf.id;

        ui.heading("Task");
        dirty |= ui.text_edit_singleline(&mut buf.title).changed();

        ui.label("Notes");
        dirty |= ui.text_edit_multiline(&mut buf.notes).changed();

        ui.separator();

        egui::ComboBox::from_label("Project")
            .selected_text(
                buf.project_id
                    .and_then(|id| projects.iter().find(|p| p.id == id))
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "Inbox".to_string()),
            )
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(buf.project_id.is_none(), "Inbox")
                    .clicked()
                {
                    buf.project_id = None;
                    dirty = true;
                }
                for project in &projects {
                    let selected = buf.project_id == Some(project.id);
                    if ui.selectable_label(selected, &project.name).clicked() {
                        buf.project_id = Some(project.id);
                        dirty = true;
                    }
                }
            });

        dirty |= date_field(ui, "Due date", &mut buf.due_date, true);
        dirty |= date_field(ui, "Defer date", &mut buf.defer_date, false);
        dirty |= recurrence_field(ui, &mut buf.recurrence, &mut buf.due_date);
        dirty |= estimate_field(ui, &mut buf.estimated_minutes);

        let mut completed = buf.completed;
        if ui.checkbox(&mut completed, "Completed").changed() {
            toggle_completed = Some(completed);
        }

        ui.separator();
        if ui.button("Delete Task").clicked() {
            delete_clicked = true;
        }

        task_id
    };

    if delete_clicked {
        state.delete_task(task_id);
        return;
    }
    if dirty {
        state.save_task_edits();
    }
    if let Some(completed) = toggle_completed {
        state.toggle_complete(task_id, completed);
    }
}
