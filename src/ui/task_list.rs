use chrono::Local;
use eframe::egui;
use egui_extras::{Column, TableBuilder};

use crate::domain::project::ProjectId;
use crate::domain::task::TaskId;
use crate::schedule::{self, ScheduleRow};
use crate::state::{
    AppState, Perspective, Selection, SortDirection, TaskSortKey, project_display_name,
};
use crate::ui::theme;

/// Read-only "Today's Events" block sourced from a connected Google
/// Calendar (see `AppState::google_calendar_config`/`run_calendar_sync`).
/// Superseded by [`draw_today_schedule`] once the first fetch has landed —
/// this only covers the brief window before that, where a "syncing…" /
/// error line is still worth showing above the plain task table.
fn draw_calendar_events(ui: &mut egui::Ui, state: &AppState) {
    if state.perspective != Perspective::Today || state.google_calendar_config.is_none() {
        return;
    }
    let now = Local::now();
    let current_events: Vec<_> = state
        .calendar_events
        .iter()
        .filter(|e| schedule::is_current(e, now))
        .collect();
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.strong("Today's Events");
            if state.calendar_busy {
                ui.weak("(syncing…)");
            }
        });
        if let Some(status) = &state.calendar_status {
            ui.colored_label(theme::OVERDUE, status);
        } else if current_events.is_empty() {
            ui.weak("No events today.");
        }
        for event in current_events {
            ui.horizontal(|ui| {
                let time_label = if event.all_day {
                    "All day".to_string()
                } else {
                    event
                        .start
                        .with_timezone(&Local)
                        .format("%-I:%M %p")
                        .to_string()
                };
                ui.monospace(time_label);
                ui.label(&event.title);
                if let Some(location) = &event.location {
                    ui.weak(location);
                }
            });
        }
    });
    ui.add_space(8.0);
}

/// A clickable column header that doubles as the current sort indicator:
/// plain text normally, with a small triangle appended when `key` is the
/// active sort. Clicking sorts by `key`, or flips direction if it's already
/// active (see `AppState::set_sort`).
///
/// The triangle is painted as a vector shape (mirroring how egui itself
/// draws the `ComboBox`/`CollapsingHeader` arrows) rather than set as a
/// Unicode glyph like ▲/▼ — those aren't in the default egui font and show
/// up as a missing-glyph box.
fn sort_header(
    ui: &mut egui::Ui,
    label: &str,
    key: TaskSortKey,
    state: &AppState,
) -> egui::Response {
    let active = state.sort_key == key;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let mut response = ui
            .add(egui::Label::new(egui::RichText::new(label).strong()).sense(egui::Sense::click()));
        if active {
            let size = egui::vec2(8.0, 6.0);
            let (rect, arrow_response) = ui.allocate_exact_size(size, egui::Sense::click());
            if ui.is_rect_visible(rect) {
                let points = match state.sort_direction {
                    SortDirection::Ascending => {
                        vec![rect.left_bottom(), rect.right_bottom(), rect.center_top()]
                    }
                    SortDirection::Descending => {
                        vec![rect.left_top(), rect.right_top(), rect.center_bottom()]
                    }
                };
                ui.painter().add(egui::Shape::convex_polygon(
                    points,
                    ui.visuals().strong_text_color(),
                    egui::Stroke::NONE,
                ));
            }
            response = response.union(arrow_response);
        }
        response
    })
    .inner
}

/// The width to give the Project column, measured from the longest visible
/// project name (plus the header). `Column::auto()` only converges to the
/// right width after a couple of frames (it sizes off last frame's measured
/// content), and this app only repaints on input, so a narrower project
/// name after switching perspectives could stay stuck at the previous,
/// wider size until some unrelated repaint nudged it. Measuring here keeps
/// it correct in the same frame the content changes.
fn project_col_width(ui: &egui::Ui, state: &AppState) -> f32 {
    let project_col_min = 80.0_f32;
    let project_col_max = (ui.available_width() * 0.4).max(project_col_min);
    let font_id = egui::TextStyle::Body.resolve(ui.style());
    let widest = state
        .visible_tasks
        .iter()
        .map(|t| project_display_name(t.project_id, &state.projects))
        .chain(std::iter::once("Project".to_string()))
        .map(|text| {
            ui.painter()
                .layout_no_wrap(text, font_id.clone(), egui::Color32::WHITE)
                .size()
                .x
        })
        .fold(0.0_f32, f32::max);
    (widest + 12.0).clamp(project_col_min, project_col_max)
}

/// The Today view's auto-ordered agenda: calendar events in chronological
/// order with tasks slotted into the gaps between them by estimate (see
/// `crate::schedule` and `AppState::refresh_today_schedule`). Replaces both
/// `draw_calendar_events` and the plain task table once the calendar's
/// connected and a fetch has landed. Task rows still index into
/// `state.visible_tasks` (which `refresh_today_schedule` keeps in this same
/// order), so the checkbox, click-to-open and keyboard highlight all work
/// exactly as in the plain table; event rows are read-only.
///
/// Returns the intents the row closures collected (they only get
/// `&AppState`); `draw` applies them once its borrow ends.
fn draw_today_schedule(
    ui: &mut egui::Ui,
    state: &AppState,
) -> (Option<(TaskId, bool)>, Option<TaskId>, Option<TaskSortKey>) {
    let mut to_toggle_complete = None;
    let mut to_select = None;
    let mut sort_clicked = None;

    ui.horizontal(|ui| {
        ui.strong("Today");
        if state.calendar_busy {
            ui.weak("(syncing…)");
        }
    });
    if let Some(status) = &state.calendar_status {
        ui.colored_label(theme::OVERDUE, status);
    }
    ui.add_space(6.0);

    let today = Local::now().date_naive();
    let project_width = project_col_width(ui, state);

    TableBuilder::new(ui)
        .id_salt("today_schedule")
        .striped(true)
        .column(Column::exact(78.0)) // slot time
        .column(Column::auto()) // complete
        .column(Column::remainder()) // title
        .column(Column::auto().at_least(70.0)) // estimate
        .column(Column::auto().at_least(60.0)) // priority
        .column(Column::initial(project_width)) // project
        .header(28.0, |mut header| {
            header.col(|ui| {
                ui.strong("Time");
            });
            header.col(|ui| {
                ui.strong("");
            });
            header.col(|ui| {
                ui.strong("Title");
            });
            header.col(|ui| {
                if sort_header(ui, "Estimate", TaskSortKey::Estimate, state).clicked() {
                    sort_clicked = Some(TaskSortKey::Estimate);
                }
            });
            header.col(|ui| {
                if sort_header(ui, "Priority", TaskSortKey::Priority, state).clicked() {
                    sort_clicked = Some(TaskSortKey::Priority);
                }
            });
            header.col(|ui| {
                if sort_header(ui, "Project", TaskSortKey::Project, state).clicked() {
                    sort_clicked = Some(TaskSortKey::Project);
                }
            });
        })
        .body(|body| {
            body.rows(28.0, state.today_schedule.len(), |mut row| {
                match state.today_schedule[row.index()] {
                    ScheduleRow::Event { index } => {
                        let Some(event) = state.calendar_events.get(index) else {
                            return;
                        };
                        row.col(|ui| {
                            let label = if event.all_day {
                                "All day".to_string()
                            } else {
                                event
                                    .start
                                    .with_timezone(&Local)
                                    .format("%-I:%M %p")
                                    .to_string()
                            };
                            ui.monospace(egui::RichText::new(label).color(theme::ACCENT));
                        });
                        row.col(|_ui| {});
                        row.col(|ui| {
                            ui.label(egui::RichText::new(&event.title).color(theme::ACCENT));
                            if let Some(location) = &event.location {
                                ui.weak(location);
                            }
                        });
                        row.col(|_ui| {});
                        row.col(|_ui| {});
                        row.col(|_ui| {});
                    }
                    ScheduleRow::Task { index, start } => {
                        let Some(task) = state.visible_tasks.get(index) else {
                            return;
                        };
                        let task_id = task.id;
                        let mut completed = task.completed;
                        let overdue = task.due_date.is_some_and(|d| d < today) && !completed;
                        let estimate = task
                            .estimated_minutes
                            .map(crate::ui::format_estimate)
                            .unwrap_or_default();
                        let priority = task
                            .priority
                            .map(|p| p.to_string())
                            .unwrap_or_default();
                        let project = project_display_name(task.project_id, &state.projects);
                        let is_highlighted = state.highlighted_task == Some(task_id);
                        let is_open = state.selection == Selection::Task(task_id);

                        row.set_selected(is_highlighted);

                        row.col(|ui| {
                            let label = start
                                .map(|t| t.format("%-I:%M %p").to_string())
                                .unwrap_or_default();
                            ui.weak(egui::RichText::new(label).monospace());
                        });
                        row.col(|ui| {
                            if ui.checkbox(&mut completed, "").changed() {
                                to_toggle_complete = Some((task_id, completed));
                            }
                        });
                        row.col(|ui| {
                            let title = if task.recurrence.is_some() {
                                format!("\u{21bb} {}", task.title)
                            } else {
                                task.title.clone()
                            };
                            let mut text = egui::RichText::new(title);
                            if completed {
                                text = text.strikethrough().weak();
                            } else if overdue {
                                text = text.color(theme::OVERDUE);
                            }
                            let label = ui.selectable_label(is_open, text);
                            let label = match &task.recurrence {
                                Some(r) => label.on_hover_text(format!("Repeats {}", r.describe())),
                                None => label,
                            };
                            if label.clicked() {
                                to_select = Some(task_id);
                            }
                        });
                        row.col(|ui| {
                            ui.weak(estimate);
                        });
                        row.col(|ui| {
                            ui.weak(priority);
                        });
                        row.col(|ui| {
                            ui.weak(project);
                        });
                    }
                }
            });
        });

    (to_toggle_complete, to_select, sort_clicked)
}

pub fn draw(ui: &mut egui::Ui, state: &mut AppState) {
    let today_agenda = state.perspective == Perspective::Today
        && state.google_calendar_config.is_some()
        && !state.today_schedule.is_empty();

    let mut to_toggle_complete: Option<(TaskId, bool)> = None;
    let mut to_select: Option<TaskId> = None;
    let mut sort_clicked: Option<TaskSortKey> = None;

    if today_agenda {
        (to_toggle_complete, to_select, sort_clicked) = draw_today_schedule(ui, state);
    } else {
        draw_calendar_events(ui, state);

        if state.perspective == Perspective::Completed && !state.visible_tasks.is_empty() {
            ui.horizontal(|ui| {
                if ui.button("Archive Completed").clicked() {
                    state.open_archive_confirm();
                }
            });
            ui.add_space(4.0);
        }

        let today = Local::now().date_naive();
        let project_col_width = project_col_width(ui, state);

        let project_name = |id: Option<ProjectId>, state: &AppState| -> String {
            project_display_name(id, &state.projects)
        };

        TableBuilder::new(ui)
            .striped(true)
            .column(Column::auto()) // complete
            .column(Column::remainder()) // title
            .column(Column::auto().at_least(90.0)) // due date
            .column(Column::auto().at_least(70.0)) // estimate
            .column(Column::auto().at_least(60.0)) // priority
            .column(Column::initial(project_col_width)) // project
            .header(28.0, |mut header| {
                header.col(|ui| {
                    ui.strong("");
                });
                header.col(|ui| {
                    ui.strong("Title");
                });
                header.col(|ui| {
                    if sort_header(ui, "Due", TaskSortKey::DueDate, state).clicked() {
                        sort_clicked = Some(TaskSortKey::DueDate);
                    }
                });
                header.col(|ui| {
                    if sort_header(ui, "Estimate", TaskSortKey::Estimate, state).clicked() {
                        sort_clicked = Some(TaskSortKey::Estimate);
                    }
                });
                header.col(|ui| {
                    if sort_header(ui, "Priority", TaskSortKey::Priority, state).clicked() {
                        sort_clicked = Some(TaskSortKey::Priority);
                    }
                });
                header.col(|ui| {
                    if sort_header(ui, "Project", TaskSortKey::Project, state).clicked() {
                        sort_clicked = Some(TaskSortKey::Project);
                    }
                });
            })
            .body(|body| {
                body.rows(28.0, state.visible_tasks.len(), |mut row| {
                    let task = &state.visible_tasks[row.index()];
                    let task_id = task.id;
                    let mut completed = task.completed;
                    let title = task.title.clone();
                    let overdue = task.due_date.is_some_and(|d| d < today) && !completed;
                    let due = task
                        .due_date
                        .map(|d| d.format("%Y-%m-%d").to_string())
                        .unwrap_or_default();
                    let estimate = task
                        .estimated_minutes
                        .map(crate::ui::format_estimate)
                        .unwrap_or_default();
                    let priority = task.priority.map(|p| p.to_string()).unwrap_or_default();
                    let project = project_name(task.project_id, state);
                    let is_highlighted = state.highlighted_task == Some(task_id);
                    let is_open = state.selection == Selection::Task(task_id);

                    row.set_selected(is_highlighted);

                    row.col(|ui| {
                        if ui.checkbox(&mut completed, "").changed() {
                            to_toggle_complete = Some((task_id, completed));
                        }
                    });
                    row.col(|ui| {
                        let title = if task.recurrence.is_some() {
                            format!("\u{21bb} {title}")
                        } else {
                            title
                        };
                        let mut text = egui::RichText::new(title);
                        if completed {
                            text = text.strikethrough().weak();
                        }
                        let label = ui.selectable_label(is_open, text);
                        let label = match &task.recurrence {
                            Some(r) => label.on_hover_text(format!("Repeats {}", r.describe())),
                            None => label,
                        };
                        if label.clicked() {
                            to_select = Some(task_id);
                        }
                    });
                    row.col(|ui| {
                        let text = if overdue {
                            egui::RichText::new(due).color(theme::OVERDUE)
                        } else {
                            egui::RichText::new(due)
                        };
                        ui.label(text);
                    });
                    row.col(|ui| {
                        ui.weak(estimate);
                    });
                    row.col(|ui| {
                        ui.weak(priority);
                    });
                    row.col(|ui| {
                        ui.weak(project);
                    });
                });
            });
    }

    if let Some((id, completed)) = to_toggle_complete {
        state.toggle_complete(id, completed);
    }
    if let Some(id) = to_select {
        state.select_task(id);
    }
    if let Some(key) = sort_clicked {
        state.set_sort(key);
    }
}
