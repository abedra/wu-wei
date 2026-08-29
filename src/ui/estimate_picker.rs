use eframe::egui;

use crate::state::{self, AppState};

/// Fixed id for the "type an estimate" field, so the shortcut layer can tell
/// when it holds keyboard focus (mirrors `due_date_picker::field_id`).
pub fn field_id() -> egui::Id {
    egui::Id::new("estimate_picker_text_field")
}

/// Draws the "Set Estimate" overlay when a picker is open. Takes the whole
/// `Context` (not a `Ui`) since it renders as a floating `Window`, independent
/// of whichever panel is currently being laid out.
pub fn draw(ctx: &egui::Context, state: &mut AppState) {
    let Some((highlighted, error)) = state
        .estimate_picker
        .as_ref()
        .map(|p| (p.highlighted, p.error.clone()))
    else {
        return;
    };
    let options = state::estimate_picker_options();

    // Edited locally, then written back into the picker after the window
    // closure — a `&mut state.estimate_picker` borrow can't span the
    // `Window::show` call.
    let mut text_input = state
        .estimate_picker
        .as_ref()
        .map(|p| p.text_input.clone())
        .unwrap_or_default();

    let mut clicked_index: Option<usize> = None;
    let mut submit_text = false;
    egui::Window::new("Set Estimate")
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

            ui.separator();
            ui.label("…or type one:");
            let field = ui.add(
                egui::TextEdit::singleline(&mut text_input)
                    .id(field_id())
                    .hint_text("e.g. 90, 45m, 1h30m, 2h")
                    .desired_width(f32::INFINITY),
            );
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit_text = true;
            }

            if let Some(err) = &error {
                ui.colored_label(crate::ui::theme::WARM, err);
            }
        });

    if let Some(picker) = state.estimate_picker.as_mut() {
        picker.text_input = text_input;
    }

    if let Some(index) = clicked_index {
        state.pick_estimate_in_picker(index);
    } else if submit_text {
        state.submit_estimate_picker_text();
    }
}
