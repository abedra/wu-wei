use chrono::Local;
use eframe::egui;
use egui_extras::{Column, TableBuilder};

use crate::domain::project::ProjectId;
use crate::domain::task::TaskId;
use crate::state::{AppState, Perspective, Selection};
use crate::ui::theme;

pub fn draw(ui: &mut egui::Ui, state: &mut AppState) {
    if state.perspective == Perspective::Completed && !state.visible_tasks.is_empty() {
        ui.horizontal(|ui| {
            if ui.button("Archive Completed").clicked() {
                state.open_archive_confirm();
            }
        });
        ui.add_space(4.0);
    }

    let project_name = |id: Option<ProjectId>, state: &AppState| -> String {
        match id {
            None => "Inbox".to_string(),
            Some(pid) => state
                .projects
                .iter()
                .find(|p| p.id == pid)
                .map(|p| p.name.clone())
                .unwrap_or_default(),
        }
    };

    let mut to_toggle_complete: Option<(TaskId, bool)> = None;
    let mut to_select: Option<TaskId> = None;
    let today = Local::now().date_naive();

    // `Column::auto()` only converges to the right width after a couple of
    // frames (it sizes off of last frame's measured content), and this app
    // only repaints on input, so a narrower project name after switching
    // perspectives could stay stuck at the previous, wider size until some
    // unrelated repaint happened to nudge it. Measuring the text ourselves
    // and handing the table an exact width keeps it correct in the same
    // frame the content changes, and still shrinks/grows with the longest
    // visible project name.
    let project_col_min = 80.0_f32;
    let project_col_max = (ui.available_width() * 0.4).max(project_col_min);
    let font_id = egui::TextStyle::Body.resolve(ui.style());
    let project_text_width = state
        .visible_tasks
        .iter()
        .map(|t| project_name(t.project_id, state))
        .chain(std::iter::once("Project".to_string()))
        .map(|text| {
            ui.painter()
                .layout_no_wrap(text, font_id.clone(), egui::Color32::WHITE)
                .size()
                .x
        })
        .fold(0.0_f32, f32::max);
    let project_col_width = (project_text_width + 12.0).clamp(project_col_min, project_col_max);

    TableBuilder::new(ui)
        .striped(true)
        .column(Column::auto()) // complete
        .column(Column::remainder()) // title
        .column(Column::auto().at_least(90.0)) // due date
        .column(Column::initial(project_col_width)) // project
        .header(28.0, |mut header| {
            header.col(|ui| {
                ui.strong("");
            });
            header.col(|ui| {
                ui.strong("Title");
            });
            header.col(|ui| {
                ui.strong("Due");
            });
            header.col(|ui| {
                ui.strong("Project");
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
                let project = project_name(task.project_id, state);
                // Keyboard cursor position (Up/Down), separate from whether
                // details are open (mirrors OmniFocus: navigating doesn't open
                // the inspector by itself).
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
                    if ui.selectable_label(is_open, text).clicked() {
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
                    ui.weak(project);
                });
            });
        });

    if let Some((id, completed)) = to_toggle_complete {
        state.toggle_complete(id, completed);
    }
    if let Some(id) = to_select {
        state.select_task(id);
    }
}
