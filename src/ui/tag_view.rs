use eframe::egui;

use crate::domain::tag::TagId;
use crate::state::AppState;

pub fn draw_list(ui: &mut egui::Ui, state: &mut AppState) {
    ui.heading("Tags");
    ui.horizontal(|ui| {
        let response = ui.text_edit_singleline(&mut state.new_tag_name);
        let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (response.lost_focus() && enter_pressed) || ui.button("New Tag").clicked() {
            state.create_tag();
            response.request_focus();
        }
    });

    ui.separator();

    let mut to_select: Option<TagId> = None;
    for tag in &state.tags {
        if ui.selectable_label(false, &tag.name).clicked() {
            to_select = Some(tag.id);
        }
    }
    if let Some(id) = to_select {
        state.select_tag(id);
    }
}

pub fn draw_detail(ui: &mut egui::Ui, state: &mut AppState) {
    if state.tag_edit_buffer.is_none() {
        ui.label("Select a tag.");
        return;
    }

    let mut dirty = false;
    let mut delete_clicked = false;
    let tag_id = {
        let buf = state.tag_edit_buffer.as_mut().unwrap();
        let tag_id = buf.id;

        ui.heading("Tag");
        dirty |= ui.text_edit_singleline(&mut buf.name).changed();

        ui.separator();
        if ui.button("Delete Tag").clicked() {
            delete_clicked = true;
        }

        tag_id
    };

    if delete_clicked {
        state.delete_tag(tag_id);
        return;
    }
    if dirty {
        state.save_tag_edits();
    }
}
