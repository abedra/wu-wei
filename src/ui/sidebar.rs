use eframe::egui;

use crate::state::{AppState, Perspective};

pub fn draw(ui: &mut egui::Ui, state: &mut AppState, logo: &egui::TextureHandle) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let heading_height = ui.text_style_height(&egui::TextStyle::Heading);
        ui.add(
            egui::Image::from_texture(logo)
                .fit_to_exact_size(egui::vec2(heading_height, heading_height)),
        )
        .on_hover_text("Wu Wei");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // The info button: shows/hides the detail panel regardless of
            // selection (it otherwise opens and closes automatically).
            if ui
                .selectable_label(state.detail_panel_open, "Details")
                .on_hover_text("Toggle the detail panel")
                .clicked()
            {
                state.toggle_detail_panel();
            }
        });
    });
    ui.add_space(8.0);

    let entries = state.sidebar_entries();
    let mut shown_projects_label = false;
    for (index, perspective) in entries.iter().copied().enumerate() {
        if matches!(perspective, Perspective::Project(_)) && !shown_projects_label {
            section_label(ui, "Projects");
            shown_projects_label = true;
        }

        let label = label_for(state, perspective);
        let selected = state.perspective == perspective;
        let response = ui.selectable_label(selected, label);
        if response.clicked() {
            state.focus_sidebar(index);
        }
        if let Perspective::Project(id) = perspective {
            response.context_menu(|ui| {
                if ui.button("Edit Project...").clicked() {
                    state.select_project(id);
                    ui.close();
                }
            });
        }
    }
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(10.0);
    ui.label(egui::RichText::new(text.to_uppercase()).small().weak());
    ui.add_space(2.0);
}

fn label_for(state: &AppState, perspective: Perspective) -> String {
    match perspective {
        Perspective::Inbox => "Inbox".to_string(),
        Perspective::Today => "Today".to_string(),
        Perspective::Overdue => "Overdue".to_string(),
        Perspective::Completed => "Completed".to_string(),
        Perspective::Project(id) => state
            .projects
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.name.clone())
            .unwrap_or_default(),
    }
}
