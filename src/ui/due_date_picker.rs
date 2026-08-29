use chrono::Local;
use eframe::egui;

use crate::state::{self, AppState};

/// Fixed id for the "type a date" field, so the shortcut layer can tell when
/// it holds keyboard focus (mirrors `quick_capture::field_id`).
pub fn field_id() -> egui::Id {
    egui::Id::new("due_date_picker_text_field")
}

/// Draws the "Set Due Date" overlay when a picker is open. Takes the whole
/// `Context` (not a `Ui`) since it renders as a floating `Window`, independent
/// of whichever panel is currently being laid out.
pub fn draw(ctx: &egui::Context, state: &mut AppState) {
    let Some((highlighted, ai_busy, ai_error)) = state
        .due_date_picker
        .as_ref()
        .map(|p| (p.highlighted, p.ai_pending.is_some(), p.ai_error.clone()))
    else {
        return;
    };
    let options = state::due_date_picker_options(Local::now().date_naive());

    // Edited locally, then written back into the picker after the window
    // closure — a `&mut state.due_date_picker` borrow can't span the
    // `Window::show` call, which also needs `state` for the option list.
    let mut text_input = state
        .due_date_picker
        .as_ref()
        .map(|p| p.text_input.clone())
        .unwrap_or_default();

    let mut clicked_index: Option<usize> = None;
    let mut submit_text = false;
    egui::Window::new("Set Due Date")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label("Up/Down to choose - Enter to set - Esc to cancel");
            ui.separator();
            ui.add_enabled_ui(!ai_busy, |ui| {
                for (i, (label, _)) in options.iter().enumerate() {
                    if ui.selectable_label(highlighted == i, label).clicked() {
                        clicked_index = Some(i);
                    }
                }
            });

            ui.separator();
            ui.label("…or type a date and let AI work it out:");
            let field = ui.add_enabled(
                !ai_busy,
                egui::TextEdit::singleline(&mut text_input)
                    .id(field_id())
                    .hint_text("e.g. next friday, in 3 weeks, end of month")
                    .desired_width(f32::INFINITY),
            );
            if !ai_busy && field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit_text = true;
            }

            if ai_busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("Working out that date…");
                });
            } else if let Some(err) = &ai_error {
                ui.colored_label(crate::ui::theme::WARM, err);
            }
        });

    if let Some(picker) = state.due_date_picker.as_mut() {
        picker.text_input = text_input;
    }

    if let Some(index) = clicked_index {
        state.pick_due_date_in_picker(index);
    } else if submit_text {
        state.submit_due_date_picker_text();
    }
}
