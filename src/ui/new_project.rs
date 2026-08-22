use eframe::egui;

use crate::state::AppState;

/// Fixed id so `draw` can request focus on this field immediately when the
/// popup opens (see `AppState::new_project_focus_pending`).
pub fn field_id() -> egui::Id {
    egui::Id::new("new_project_field")
}

/// Draws the new-project popup when open (toggled by Cmd+Shift+N, see
/// `ui::shortcuts`). Same floating-window shape as `ui::quick_capture`.
pub fn draw(ctx: &egui::Context, state: &mut AppState) {
    if !state.new_project_popup_open {
        return;
    }

    egui::Window::new("New Project")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 60.0))
        .show(ctx, |ui| {
            ui.label("Project name:");
            let response =
                ui.add(egui::TextEdit::singleline(&mut state.new_project_name).id(field_id()));
            if state.new_project_focus_pending {
                state.new_project_focus_pending = false;
                response.request_focus();
            }
            ui.label("Enter to add - Esc to cancel");
        });
}
