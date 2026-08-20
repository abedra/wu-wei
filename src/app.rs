use eframe::egui;
use rusqlite::Connection;

use crate::state::{AppState, Selection};
use crate::ui;

pub struct WuWeiApp {
    state: AppState,
    icon: egui::TextureHandle,
}

impl WuWeiApp {
    pub fn new(cc: &eframe::CreationContext<'_>, conn: Connection) -> Self {
        const LOGO_SIZE: u32 = 128;
        let rgba = ui::icon::enso_rgba(LOGO_SIZE, ui::theme::ACCENT);
        let image = egui::ColorImage::from_rgba_unmultiplied([LOGO_SIZE as usize; 2], &rgba);
        let icon = cc
            .egui_ctx
            .load_texture("app_logo", image, egui::TextureOptions::LINEAR);

        Self {
            state: AppState::new(conn),
            icon,
        }
    }
}

impl eframe::App for WuWeiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.state.poll_llm();
        self.state.poll_chat();
        ui::shortcuts::handle(&ctx, &mut self.state);
        ui::project_picker::draw(&ctx, &mut self.state);
        ui::due_date_picker::draw(&ctx, &mut self.state);
        ui::quick_capture::draw(&ctx, &mut self.state);
        ui::new_project::draw(&ctx, &mut self.state);

        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(200.0)
            .frame(ui::theme::sidebar_frame(ui.style()))
            .show(ui, |ui| ui::sidebar::draw(ui, &mut self.state, &self.icon));

        if self.state.detail_panel_open {
            egui::Panel::right("detail")
                .resizable(true)
                .default_size(320.0)
                .show(ui, |ui| match self.state.selection {
                    Selection::Task(_) => ui::task_detail::draw(ui, &mut self.state),
                    Selection::Project(_) => ui::project_view::draw_detail(ui, &mut self.state),
                    Selection::None => {
                        ui.label("Select a task or project.");
                    }
                });
        }

        egui::Panel::bottom("ai_chat")
            .resizable(true)
            .default_size(180.0)
            .frame(ui::theme::chat_frame(ui.style()))
            .show(ui, |ui| ui::ai_chat::draw(ui, &mut self.state));

        if let Some(msg) = self.state.error_message.clone() {
            egui::Panel::bottom("error_bar").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(ui.visuals().error_fg_color, msg);
                    if ui.button("Dismiss").clicked() {
                        self.state.error_message = None;
                    }
                });
            });
        }

        egui::CentralPanel::default().show(ui, |ui| ui::task_list::draw(ui, &mut self.state));
    }
}
