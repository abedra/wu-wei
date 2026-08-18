use eframe::egui;
use rusqlite::Connection;

use crate::state::{AppState, Perspective, Selection};
use crate::ui;

pub struct LoaApp {
    state: AppState,
}

impl LoaApp {
    pub fn new(conn: Connection) -> Self {
        Self {
            state: AppState::new(conn),
        }
    }
}

impl eframe::App for LoaApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.state.poll_llm();
        ui::shortcuts::handle(&ctx, &mut self.state);
        ui::project_picker::draw(&ctx, &mut self.state);
        ui::due_date_picker::draw(&ctx, &mut self.state);
        ui::quick_capture::draw(&ctx, &mut self.state);

        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(200.0)
            .frame(ui::theme::sidebar_frame(ui.style()))
            .show(ui, |ui| ui::sidebar::draw(ui, &mut self.state));

        if self.state.detail_panel_open {
            egui::Panel::right("detail")
                .resizable(true)
                .default_size(320.0)
                .show(ui, |ui| match self.state.selection {
                    Selection::Task(_) => ui::task_detail::draw(ui, &mut self.state),
                    Selection::Project(_) => ui::project_view::draw_detail(ui, &mut self.state),
                    Selection::Tag(_) => ui::tag_view::draw_detail(ui, &mut self.state),
                    Selection::None => {
                        ui.label("Select a task, project, or tag.");
                    }
                });
        }

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

        egui::CentralPanel::default().show(ui, |ui| match self.state.perspective {
            Perspective::AllProjects => ui::project_view::draw_list(ui, &mut self.state),
            Perspective::AllTags => ui::tag_view::draw_list(ui, &mut self.state),
            _ => ui::task_list::draw(ui, &mut self.state),
        });
    }
}
