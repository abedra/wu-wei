use chrono::Local;
use eframe::egui;

use crate::state::{self, AppState};

/// Draws the "Set Due Date" overlay when a picker is open. Takes the whole
/// `Context` (not a `Ui`) since it renders as a floating `Window`, independent
/// of whichever panel is currently being laid out.
pub fn draw(ctx: &egui::Context, state: &mut AppState) {
    let Some(highlighted) = state.due_date_picker.as_ref().map(|p| p.highlighted) else {
        return;
    };
    let options = state::due_date_picker_options(Local::now().date_naive());

    let mut clicked_index: Option<usize> = None;
    egui::Window::new("Set Due Date")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label("Up/Down to choose - Enter to set - Esc to cancel");
            ui.separator();
            for (i, (label, _)) in options.iter().enumerate() {
                if ui.selectable_label(highlighted == i, label).clicked() {
                    clicked_index = Some(i);
                }
            }
        });

    if let Some(index) = clicked_index {
        state.pick_due_date_in_picker(index);
    }
}
