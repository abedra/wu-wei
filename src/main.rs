mod app;
mod db;
mod db_bootstrap;
mod desktop_install;
mod domain;
mod llm;
mod state;
mod sync;
mod ui;

use app::WuWeiApp;

fn main() -> eframe::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("install-desktop") {
        let exe = std::env::current_exe()
            .expect("failed to resolve the current executable's path")
            .to_string_lossy()
            .into_owned();
        desktop_install::run(&exe);
        return Ok(());
    }

    let db_path = db_bootstrap::resolve_db_path();
    let conn = db::open(&db_path).expect("failed to open database");

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_app_id(desktop_install::APP_ID)
            .with_icon(ui::icon::window_icon(ui::theme::ACCENT)),
        ..Default::default()
    };

    eframe::run_native(
        "Wu Wei",
        options,
        Box::new(|cc| {
            ui::theme::apply(&cc.egui_ctx);
            Ok(Box::new(WuWeiApp::new(cc, conn)))
        }),
    )
}
