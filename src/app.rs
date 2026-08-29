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

        let mut state = AppState::new(conn);
        state.set_perspective(crate::state::Perspective::Today);

        Self { state, icon }
    }
}

impl eframe::App for WuWeiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // egui/eframe only repaints on input by default, so a window just
        // sitting open (e.g. on Today, overnight) would otherwise never get
        // another frame to notice the date changed. This re-arms a wakeup
        // each frame, which is what actually gives `refresh_if_date_changed`
        // a chance to run when nothing else is happening — a few times an
        // hour is plenty for a same-day check like this.
        ctx.request_repaint_after(std::time::Duration::from_secs(300));
        self.state.refresh_if_date_changed();
        self.state.poll_llm();
        self.state.poll_due_date_picker();
        self.state.poll_chat();
        self.state.poll_sync();
        self.state.maybe_auto_sync();
        self.state.poll_calendar();
        self.state.maybe_auto_sync_calendar();
        self.state.poll_google_auth();
        self.state.poll_db_path_dialog();
        self.state.poll_sync_folder_dialog();
        // These run on background threads (native file dialogs — see
        // `AppState::browse_for_db_path` — and `sync::run_async`) so they
        // don't freeze the window, but that means nothing else prompts a
        // repaint when one finishes. Without this poll, the resolved
        // dialog path / the "Syncing..." status wouldn't update until some
        // unrelated input (e.g. a mouse move) happened to trigger the next
        // frame. The other background operations (LLM, chat, calendar)
        // render a spinner while busy, which re-arms its own repaint.
        if self.state.db_path_dialog.is_some()
            || self.state.sync_folder_dialog.is_some()
            || self.state.sync_busy
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        ui::shortcuts::handle(&ctx, &mut self.state);
        ui::project_picker::draw(&ctx, &mut self.state);
        ui::due_date_picker::draw(&ctx, &mut self.state);
        ui::estimate_picker::draw(&ctx, &mut self.state);
        ui::quick_capture::draw(&ctx, &mut self.state);
        ui::new_project::draw(&ctx, &mut self.state);
        ui::settings::draw(&ctx, &mut self.state);
        ui::archive_confirm::draw(&ctx, &mut self.state);

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
