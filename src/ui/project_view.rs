use eframe::egui;

use crate::domain::project::{ProjectId, ProjectKind, ProjectStatus};
use crate::state::AppState;

pub fn draw_list(ui: &mut egui::Ui, state: &mut AppState) {
    ui.heading("Projects");
    ui.horizontal(|ui| {
        let response = ui.text_edit_singleline(&mut state.new_project_name);
        let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (response.lost_focus() && enter_pressed) || ui.button("New Project").clicked() {
            state.create_project();
            response.request_focus();
        }
    });

    ui.separator();

    let mut to_select: Option<ProjectId> = None;
    for project in &state.projects {
        ui.horizontal(|ui| {
            if ui.selectable_label(false, &project.name).clicked() {
                to_select = Some(project.id);
            }
            ui.label(format!("[{}]", project.status.label()));
            ui.label(format!("[{}]", project.kind.label()));
        });
    }
    if let Some(id) = to_select {
        state.select_project(id);
    }
}

pub fn draw_detail(ui: &mut egui::Ui, state: &mut AppState) {
    if state.project_edit_buffer.is_none() {
        ui.label("Select a project.");
        return;
    }

    let mut dirty = false;
    let mut delete_clicked = false;
    let project_id = {
        let buf = state.project_edit_buffer.as_mut().unwrap();
        let project_id = buf.id;

        ui.heading("Project");
        dirty |= ui.text_edit_singleline(&mut buf.name).changed();

        ui.label("Notes");
        dirty |= ui.text_edit_multiline(&mut buf.notes).changed();

        ui.separator();

        egui::ComboBox::from_label("Status")
            .selected_text(buf.status.label())
            .show_ui(ui, |ui| {
                for status in ProjectStatus::all() {
                    if ui
                        .selectable_label(buf.status == status, status.label())
                        .clicked()
                    {
                        buf.status = status;
                        dirty = true;
                    }
                }
            });

        egui::ComboBox::from_label("Kind")
            .selected_text(buf.kind.label())
            .show_ui(ui, |ui| {
                for kind in ProjectKind::all() {
                    if ui
                        .selectable_label(buf.kind == kind, kind.label())
                        .clicked()
                    {
                        buf.kind = kind;
                        dirty = true;
                    }
                }
            });

        ui.separator();
        if ui.button("Delete Project").clicked() {
            delete_clicked = true;
        }

        project_id
    };

    if delete_clicked {
        state.delete_project(project_id);
        return;
    }
    if dirty {
        state.save_project_edits();
    }
}
