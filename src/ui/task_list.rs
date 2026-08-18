use eframe::egui;
use egui_extras::{Column, TableBuilder};

use crate::domain::project::ProjectId;
use crate::domain::task::TaskId;
use crate::state::{AppState, Selection};

pub fn draw(ui: &mut egui::Ui, state: &mut AppState) {
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
    let mut to_toggle_flag: Option<(TaskId, bool)> = None;
    let mut to_select: Option<TaskId> = None;

    TableBuilder::new(ui)
        .striped(true)
        .column(Column::auto()) // complete
        .column(Column::auto()) // flag
        .column(Column::remainder()) // title
        .column(Column::auto().at_least(90.0)) // due date
        .column(Column::auto().at_least(80.0)) // project
        .header(20.0, |mut header| {
            header.col(|ui| {
                ui.strong("");
            });
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
            body.rows(20.0, state.visible_tasks.len(), |mut row| {
                let task = &state.visible_tasks[row.index()];
                let task_id = task.id;
                let mut completed = task.completed;
                let flagged = task.flagged;
                let title = task.title.clone();
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
                    let flag_label = if flagged { "\u{2691}" } else { "\u{2690}" };
                    if ui.selectable_label(flagged, flag_label).clicked() {
                        to_toggle_flag = Some((task_id, !flagged));
                    }
                });
                row.col(|ui| {
                    if ui.selectable_label(is_open, &title).clicked() {
                        to_select = Some(task_id);
                    }
                });
                row.col(|ui| {
                    ui.label(due);
                });
                row.col(|ui| {
                    ui.label(project);
                });
            });
        });

    if let Some((id, completed)) = to_toggle_complete {
        state.toggle_complete(id, completed);
    }
    if let Some((id, flagged)) = to_toggle_flag {
        state.toggle_flag(id, flagged);
    }
    if let Some(id) = to_select {
        state.select_task(id);
    }
}
