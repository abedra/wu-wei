mod app;
mod db;
mod domain;
mod llm;
mod state;
mod ui;

use app::WuWeiApp;

fn main() -> eframe::Result<()> {
    let db_path = std::env::var("WU_WEI_DB_PATH").unwrap_or_else(|_| "wu_wei.db".to_string());
    let conn = db::open(&db_path).expect("failed to open database");

    eframe::run_native(
        "Wu Wei",
        eframe::NativeOptions::default(),
        Box::new(|cc| {
            ui::theme::apply(&cc.egui_ctx);
            Ok(Box::new(WuWeiApp::new(conn)))
        }),
    )
}
