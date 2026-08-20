use eframe::egui;

use crate::state::{AppState, Perspective};

/// Fixed id so [`crate::ui::shortcuts`] can request focus on this field
/// immediately when the popup opens.
pub fn field_id() -> egui::Id {
    egui::Id::new("quick_capture_field")
}

fn target_label(state: &AppState) -> String {
    match state.perspective {
        Perspective::Project(id) => state
            .projects
            .iter()
            .find(|p| p.id == id)
            .map(|p| format!("New task (in {}):", p.name))
            .unwrap_or_else(|| "New task:".to_string()),
        _ => "New task:".to_string(),
    }
}

/// Draws the quick-capture popup when open (toggled by Cmd+N, see
/// `ui::shortcuts`). It isn't part of the normal panel layout, so it never
/// sits in the Tab order when closed.
pub fn draw(ctx: &egui::Context, state: &mut AppState) {
    if !state.quick_capture_open {
        return;
    }
    let label = target_label(state);

    let llm_available = state.llm_available();

    egui::Window::new("New Task")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 60.0))
        .show(ctx, |ui| {
            ui.label(label);
            ui.add_enabled(
                !state.llm_busy,
                egui::TextEdit::singleline(&mut state.quick_entry_buffer).id(field_id()),
            );
            if state.llm_busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("Asking the AI to parse this...");
                });
            } else if llm_available {
                ui.label(
                    "Enter to add (parsed by AI) - Shift+Enter to add literally - Esc to cancel",
                );
            } else {
                ui.label("Enter to add - Esc to cancel");
            }
        });
}
