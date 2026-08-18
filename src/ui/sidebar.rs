use eframe::egui;

use crate::state::{AppState, Perspective};

pub fn draw(ui: &mut egui::Ui, state: &mut AppState) {
    ui.heading("loa");
    ui.separator();

    let entries = state.sidebar_entries();
    for (index, perspective) in entries.iter().copied().enumerate() {
        if perspective == Perspective::AllProjects {
            ui.separator();
            ui.label("Projects");
        } else if perspective == Perspective::AllTags {
            ui.separator();
            ui.label("Tags");
        }

        let label = label_for(state, perspective);
        let selected = state.perspective == perspective;
        if ui.selectable_label(selected, label).clicked() {
            state.focus_sidebar(index);
        }
    }
}

fn label_for(state: &AppState, perspective: Perspective) -> String {
    match perspective {
        Perspective::Inbox => "Inbox".to_string(),
        Perspective::Today => "Today".to_string(),
        Perspective::Flagged => "Flagged".to_string(),
        Perspective::Completed => "Completed".to_string(),
        Perspective::AllProjects => "All Projects".to_string(),
        Perspective::Project(id) => state
            .projects
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.name.clone())
            .unwrap_or_default(),
        Perspective::AllTags => "Manage Tags".to_string(),
        Perspective::Tag(id) => state
            .tags
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.name.clone())
            .unwrap_or_default(),
    }
}
