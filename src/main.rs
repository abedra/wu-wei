mod app;
mod db;
mod domain;
mod state;
mod ui;

use app::LoaApp;

fn main() -> eframe::Result<()> {
    let db_path = std::env::var("LOA_DB_PATH").unwrap_or_else(|_| "loa.db".to_string());
    let conn = db::open(&db_path).expect("failed to open database");

    eframe::run_native(
        "loa",
        eframe::NativeOptions::default(),
        Box::new(|cc| {
            ui::theme::apply(&cc.egui_ctx);
            Ok(Box::new(LoaApp::new(conn)))
        }),
    )
}
