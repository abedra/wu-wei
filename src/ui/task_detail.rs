use chrono::{Datelike, NaiveDate};
use eframe::egui;
use egui_extras::DatePickerButton;

use crate::domain::task::{Recurrence, RecurrenceUnit};
use crate::state::AppState;

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
                    *due_date = Some(chrono::Utc::now().date_naive());
                }
                Some(Recurrence {
                    interval: 1,
                    unit: RecurrenceUnit::Weeks,
                })
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
    changed
}

fn date_field(ui: &mut egui::Ui, label: &str, date: &mut Option<NaiveDate>) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut has_date = date.is_some();
        if ui.checkbox(&mut has_date, label).changed() {
            *date = if has_date {
                Some(chrono::Utc::now().date_naive())
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
    let mut add_tag_clicked = false;
    let mut remove_tag: Option<String> = None;
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

        dirty |= date_field(ui, "Due date", &mut buf.due_date);
        dirty |= date_field(ui, "Defer date", &mut buf.defer_date);
        dirty |= recurrence_field(ui, &mut buf.recurrence, &mut buf.due_date);
        dirty |= ui.checkbox(&mut buf.flagged, "Flagged").changed();

        let mut completed = buf.completed;
        if ui.checkbox(&mut completed, "Completed").changed() {
            toggle_completed = Some(completed);
        }

        ui.separator();
        ui.label("Tags");
        ui.horizontal_wrapped(|ui| {
            for name in &buf.tag_names {
                if ui
                    .selectable_label(false, format!("{name} \u{2715}"))
                    .clicked()
                {
                    remove_tag = Some(name.clone());
                }
            }
        });
        ui.horizontal(|ui| {
            let response = ui.text_edit_singleline(&mut buf.new_tag_input);
            let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
            let add_clicked = ui.button("Add tag").clicked();
            if (response.lost_focus() && enter_pressed) || add_clicked {
                add_tag_clicked = true;
            }
        });

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
    if let Some(name) = remove_tag {
        state.remove_tag_from_edit_buffer(&name);
    }
    if add_tag_clicked {
        state.add_tag_to_edit_buffer();
    }
}
