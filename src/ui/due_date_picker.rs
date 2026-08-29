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
///
/// Layout is field-first: the free-text box sits at the top and takes keyboard
/// focus when the picker opens, so a typed phrase ("next friday") is the
/// primary path. Tab hands control to the quick-option list below (whose
/// Up/Down/Enter live in `ui::shortcuts`) and back.
pub fn draw(ctx: &egui::Context, state: &mut AppState) {
    let Some((highlighted, focus_pending, ai_busy, ai_error)) =
        state.due_date_picker.as_ref().map(|p| {
            (
                p.highlighted,
                p.focus_pending,
                p.ai_pending.is_some(),
                p.ai_error.clone(),
            )
        })
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
            ui.label("Type a date and let AI work it out:");
            let field = ui.add_enabled(
                !ai_busy,
                egui::TextEdit::singleline(&mut text_input)
                    .id(field_id())
                    .hint_text("e.g. next friday, in 3 weeks, end of month")
                    .desired_width(f32::INFINITY),
            );
            if focus_pending {
                field.request_focus();
            }
            if !ai_busy && field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit_text = true;
            }
            // Tab toggles keyboard control between the text field and the
            // quick-option list. Consumed here so egui's own focus-ring
            // traversal doesn't land on an individual option (which would
            // stop `ui::shortcuts` from driving the list with Up/Down).
            if !ai_busy
                && ui.input_mut(|i| {
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
                        || i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab)
                })
            {
                if field.has_focus() {
                    field.surrender_focus();
                } else {
                    field.request_focus();
                }
            }

            ui.separator();
            ui.add_enabled_ui(!ai_busy, |ui| {
                for (i, (label, _)) in options.iter().enumerate() {
                    if ui.selectable_label(highlighted == i, label).clicked() {
                        clicked_index = Some(i);
                    }
                }
            });

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
        picker.focus_pending = false;
    }

    if let Some(index) = clicked_index {
        state.pick_due_date_in_picker(index);
    } else if submit_text {
        state.submit_due_date_picker_text();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_open_picker() -> AppState {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "renew passport".to_string();
        state.quick_capture_submit();
        state.move_highlight(1);
        state.open_due_date_picker();
        assert!(state.due_date_picker.is_some());
        state
    }

    #[test]
    fn draws_without_panicking_and_clears_the_focus_request() {
        let mut state = state_with_open_picker();
        assert!(state.due_date_picker.as_ref().unwrap().focus_pending);

        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |ctx| {
            draw(ctx, &mut state);
        });
        output.textures_delta.clear();

        // The one-shot focus request is consumed after the first frame.
        assert!(!state.due_date_picker.as_ref().unwrap().focus_pending);
    }
}
