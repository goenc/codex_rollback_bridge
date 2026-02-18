use crate::paths;
use eframe::egui;
use std::fs;
use std::path::{Path, PathBuf};

pub fn required_font_path(project_root: &Path) -> PathBuf {
    project_root.join(paths::REQUIRED_FONT_RELATIVE_PATH)
}

pub fn apply_required_font(ctx: &egui::Context, project_root: &Path) -> Result<PathBuf, String> {
    let font_path = required_font_path(project_root);
    if !font_path.exists() {
        return Err(format!(
            "required font is missing: {}",
            font_path.to_string_lossy()
        ));
    }

    let font_bytes = fs::read(&font_path).map_err(|err| format!("font read failed: {err}"))?;
    let mut definitions = egui::FontDefinitions::default();
    definitions.font_data.insert(
        "noto_sans_jp".to_owned(),
        egui::FontData::from_owned(font_bytes).into(),
    );
    definitions
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "noto_sans_jp".to_owned());
    definitions
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, "noto_sans_jp".to_owned());

    ctx.set_fonts(definitions);
    Ok(font_path)
}
