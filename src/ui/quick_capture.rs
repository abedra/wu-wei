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
        Perspective::Tag(id) => state
            .tags
            .iter()
            .find(|t| t.id == id)
            .map(|t| format!("New task (tagged {}):", t.name))
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

    egui::Window::new("New Task")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 60.0))
        .show(ctx, |ui| {
            ui.label(label);
            ui.add(egui::TextEdit::singleline(&mut state.quick_entry_buffer).id(field_id()));
            ui.label("Enter to add - Esc to cancel");
        });
}
