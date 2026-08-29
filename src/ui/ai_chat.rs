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
    let llm_available = state.llm_available();
    let mut send_clicked = false;
    let mut apply_clicked = false;
    let mut discard_clicked = false;
    let mut weekly_summary_clicked = false;

    ui.horizontal(|ui| {
        ui.heading(
            egui::RichText::new("AI")
                .color(crate::ui::theme::ACCENT)
                .strong(),
        );
        ui.weak("Ask it to find, file, or bulk-update tasks — e.g. \"roll all of my overdue tasks to today\".");
        if ui
            .add_enabled(
                llm_available && !state.chat_busy,
                egui::Button::new("Weekly summary"),
            )
            .on_hover_text("Recap what you completed in the last 7 days (recurring chores excluded)")
            .clicked()
        {
            weekly_summary_clicked = true;
        }
    });

    // Partition the remaining space with nested panels: the input strip is
    // anchored to the bottom, and the history fills the middle. Letting inner
    // panels divide a fixed region (rather than sizing a scroll area against
    // the outer panel's `available_height`) is what keeps the resizable outer
    // panel from growing to fit the history on every repaint.
    egui::Panel::bottom("ai_chat_input")
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            // The AI proposes changes but never applies them itself (see
            // `AppState::pending_chat_actions`) — it's been wrong about
            // whether a change was even asked for often enough that nothing
            // it proposes touches the database without this click.
            if !state.pending_chat_actions.is_empty() {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let count = state.pending_chat_actions.len();
                    ui.colored_label(
                        crate::ui::theme::WARM,
                        format!(
                            "{count} pending change{}",
                            if count == 1 { "" } else { "s" }
                        ),
                    );
                    if ui.button("Apply").clicked() {
                        apply_clicked = true;
                    }
                    if ui.button("Discard").clicked() {
                        discard_clicked = true;
                    }
                });
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let enabled = llm_available && !state.chat_busy;
                let response = ui.add_enabled(
                    enabled,
                    egui::TextEdit::singleline(&mut state.chat_input)
                        .id(field_id())
                        .desired_width(f32::INFINITY),
                );
                let enter_pressed = enabled
                    && response.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.add_enabled(enabled, egui::Button::new("Send")).clicked() || enter_pressed {
                    send_clicked = true;
                }
                if state.chat_focus_requested {
                    state.chat_focus_requested = false;
                    ui.ctx().memory_mut(|m| m.request_focus(field_id()));
                }
            });
            if !llm_available {
                ui.weak("Set WU_WEI_LLM_API_KEY (or a provider-specific key) to enable AI chat.");
            }
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("ai_chat_history")
                .auto_shrink([false, false])
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
                            ChatRole::Assistant => ("AI", crate::ui::theme::WARM),
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
                                    .color(crate::ui::theme::WARM)
                                    .strong(),
                            );
                            ui.weak("thinking…");
                        });
                    }
                });
        });

    if apply_clicked {
        state.confirm_pending_chat_actions();
    }
    if discard_clicked {
        state.discard_pending_chat_actions();
    }
    if send_clicked {
        state.chat_send();
    }
    if weekly_summary_clicked {
        state.request_weekly_summary();
    }
}
