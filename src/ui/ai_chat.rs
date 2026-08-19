use eframe::egui;

use crate::llm::ChatRole;
use crate::state::AppState;

/// Fixed id so `draw` can request focus back on this field once a reply
/// finishes (see `AppState::chat_focus_requested`).
pub fn field_id() -> egui::Id {
    egui::Id::new("ai_chat_field")
}

/// Draws the full-width AI chat strip at the bottom of the window (see
/// `ui::theme::chat_frame` for its distinct styling). Always present —
/// unlike quick capture's popup, this is a persistent part of the layout —
/// but its input is disabled with an explanatory message when no LLM
/// provider is configured.
pub fn draw(ui: &mut egui::Ui, state: &mut AppState) {
    ui.horizontal(|ui| {
        ui.heading(
            egui::RichText::new("AI")
                .color(crate::ui::theme::ACCENT)
                .strong(),
        );
        ui.weak("Ask it to find, file, or bulk-update tasks — e.g. \"roll all of my overdue tasks to today\".");
    });

    egui::ScrollArea::vertical()
        .id_salt("ai_chat_history")
        .max_height(120.0)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            // Wrap to the scroll area's width rather than the panel's
            // default (unbounded) horizontal layout, which otherwise lets
            // long replies run off the edge instead of wrapping.
            ui.set_width(ui.available_width());
            if state.chat_history.is_empty() && !state.chat_busy {
                ui.weak("No messages yet.");
            }
            for turn in &state.chat_history {
                let (label, color) = match turn.role {
                    ChatRole::User => ("You", crate::ui::theme::ACCENT),
                    ChatRole::Assistant => ("AI", crate::ui::theme::FLAG),
                };
                ui.label(egui::RichText::new(label).color(color).strong());
                ui.add(egui::Label::new(&turn.content).wrap());
                ui.add_space(6.0);
            }
            if state.chat_busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        egui::RichText::new("AI")
                            .color(crate::ui::theme::FLAG)
                            .strong(),
                    );
                    ui.weak("thinking…");
                });
            }
        });

    let llm_available = state.llm_available();
    let mut send_clicked = false;
    ui.horizontal(|ui| {
        let enabled = llm_available && !state.chat_busy;
        let response = ui.add_enabled(
            enabled,
            egui::TextEdit::singleline(&mut state.chat_input)
                .id(field_id())
                .desired_width(f32::INFINITY),
        );
        let enter_pressed =
            enabled && response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.add_enabled(enabled, egui::Button::new("Send")).clicked() || enter_pressed {
            send_clicked = true;
        }
        if state.chat_focus_requested {
            state.chat_focus_requested = false;
            ui.ctx().memory_mut(|m| m.request_focus(field_id()));
        }
    });
    if !llm_available {
        ui.weak("Set LOA_LLM_API_KEY (or a provider-specific key) to enable AI chat.");
    }

    if send_clicked {
        state.chat_send();
    }
}
