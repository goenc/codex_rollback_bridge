mod app;
mod config;
mod font;
mod git;
mod material;
mod models;
mod monitor;
mod paths;

use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let native_options = eframe::NativeOptions::default();

    eframe::run_native(
        "Codex Rollback Bridge",
        native_options,
        Box::new(move |cc| {
            let font_status = font::apply_required_font(&cc.egui_ctx, &project_root);
            Ok(Box::new(app::CodexRollbackBridgeApp::new(
                project_root.clone(),
                font_status,
            )))
        }),
    )
}
