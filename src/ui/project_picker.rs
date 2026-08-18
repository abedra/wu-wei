use eframe::egui;

use crate::state::AppState;

/// Draws the "Move to Project" overlay when a picker is open. Takes the whole
/// `Context` (not a `Ui`) since it renders as a floating `Window`, independent
/// of whichever panel is currently being laid out.
pub fn draw(ctx: &egui::Context, state: &mut AppState) {
    let Some(highlighted) = state.project_picker.as_ref().map(|p| p.highlighted) else {
        return;
    };
    // Cloned so the window closure doesn't need to borrow `state` (it's used
    // again afterward to apply whatever was clicked).
    let projects = state.projects.clone();

    let mut clicked_index: Option<usize> = None;
    egui::Window::new("Move to Project")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label("Up/Down to choose - Enter to move - Esc to cancel");
            ui.separator();
            if ui.selectable_label(highlighted == 0, "Inbox").clicked() {
                clicked_index = Some(0);
            }
            for (i, project) in projects.iter().enumerate() {
                if ui
                    .selectable_label(highlighted == i + 1, &project.name)
                    .clicked()
                {
                    clicked_index = Some(i + 1);
                }
            }
        });

    if let Some(index) = clicked_index {
        state.pick_project_in_picker(index);
    }
}
