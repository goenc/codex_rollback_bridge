mod app;
mod config;
mod font;
mod git;
mod material;
mod models;
mod monitor;
mod paths;

use std::path::{Path, PathBuf};

fn main() -> eframe::Result<()> {
    let project_root = resolve_project_root();
    let native_options = eframe::NativeOptions::default();

    eframe::run_native(
        "Codex Rollback Bridge",
        native_options,
        Box::new(move |cc| {
            force_black_text_theme(&cc.egui_ctx);
            let font_status = font::apply_required_font(&cc.egui_ctx, &project_root);
            Ok(Box::new(app::CodexRollbackBridgeApp::new(
                project_root.clone(),
                font_status,
            )))
        }),
    )
}

fn force_black_text_theme(ctx: &eframe::egui::Context) {
    let text_black = eframe::egui::Color32::from_rgb(0, 0, 0);
    let panel_bg = eframe::egui::Color32::from_gray(248);

    ctx.set_theme(eframe::egui::Theme::Light);
    ctx.all_styles_mut(|style| {
        style.visuals.dark_mode = false;
        style.visuals.text_alpha_from_coverage = eframe::egui::epaint::AlphaFromCoverage::Linear;
        style.visuals.override_text_color = Some(text_black);
        style.visuals.weak_text_color = Some(text_black);
        style.visuals.widgets.noninteractive.fg_stroke.color = text_black;
        style.visuals.widgets.inactive.fg_stroke.color = text_black;
        style.visuals.widgets.hovered.fg_stroke.color = text_black;
        style.visuals.widgets.active.fg_stroke.color = text_black;
        style.visuals.widgets.open.fg_stroke.color = text_black;
        style.visuals.hyperlink_color = eframe::egui::Color32::from_rgb(0, 90, 180);
        style.visuals.warn_fg_color = eframe::egui::Color32::from_rgb(128, 96, 0);
        style.visuals.error_fg_color = eframe::egui::Color32::from_rgb(160, 0, 0);
        style.visuals.disabled_alpha = 1.0;
        style.visuals.window_fill = panel_bg;
        style.visuals.panel_fill = panel_bg;
        style.visuals.extreme_bg_color = eframe::egui::Color32::from_gray(255);
    });
}

fn resolve_project_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Some(root) = find_project_root_from(&cwd) {
        return root;
    }

    if let Ok(exe_path) = std::env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
        && let Some(root) = find_project_root_from(exe_dir)
    {
        return root;
    }

    cwd
}

fn find_project_root_from(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start.to_path_buf());
    while let Some(dir) = current {
        if dir.join("Cargo.toml").is_file() {
            return Some(dir);
        }
        current = dir.parent().map(Path::to_path_buf);
    }
    None
}
