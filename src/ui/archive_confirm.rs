use eframe::egui;

use crate::state::AppState;

/// Draws the "Archive Completed" confirmation when open (toggled by the
/// Archive button shown in the Completed view — see `ui::task_list`). Same
/// floating-window shape as the other popups; unlike most of them this one
/// exists purely because the action behind it is a permanent, bulk delete.
pub fn draw(ctx: &egui::Context, state: &mut AppState) {
    if !state.archive_confirm_open {
        return;
    }

    let count = state.visible_tasks.iter().filter(|t| t.completed).count();
    let mut confirmed = false;
    let mut cancelled = false;

    egui::Window::new("Archive Completed Tasks")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            let plural = if count == 1 { "" } else { "s" };
            ui.label(format!(
                "Permanently delete {count} completed task{plural}? This cannot be undone."
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Archive").clicked() {
                    confirmed = true;
                }
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
            });
        });

    if confirmed {
        state.confirm_archive_completed();
    } else if cancelled {
        state.close_archive_confirm();
    }
}
