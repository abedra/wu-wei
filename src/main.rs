// Rust binaries are console-subsystem by default on Windows, so launching
// the GUI normally would pop up an attached console window alongside it.
// Suppress that in release builds only — debug builds (`cargo run`/`make
// run`) keep the console so panics and eprintln! output stay visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod calendar;
mod db;
mod db_bootstrap;
mod desktop_install;
mod domain;
mod llm;
mod schedule;
mod state;
mod sync;
mod ui;

use app::WuWeiApp;

fn main() -> eframe::Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("install-desktop") => {
            let exe = std::env::current_exe()
                .expect("failed to resolve the current executable's path")
                .to_string_lossy()
                .into_owned();
            desktop_install::run(&exe);
            return Ok(());
        }
        // Used by the packaging scripts (see `packaging/`) to get icon files
        // on disk before the app itself is installed — the icons are drawn
        // procedurally, not shipped as assets.
        Some("emit-icons") => {
            let dir = std::env::args()
                .nth(2)
                .expect("usage: wu-wei emit-icons <dir>");
            desktop_install::emit_icons(&dir);
            return Ok(());
        }
        _ => {}
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
