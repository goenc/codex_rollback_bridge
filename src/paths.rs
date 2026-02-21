use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "CodexRollbackBridge";
pub const CONFIG_DIR_RELATIVE: &str = "assets/config";
pub const SETTINGS_DEFAULT_FILE: &str = "settings.default.json";
pub const SETTINGS_OVERRIDE_FILE: &str = "settings.override.json";
pub const JSON_EXTENSION: &str = ".json";
pub const REQUIRED_FONT_RELATIVE_PATH: &str = "assets/fonts/NotoSansJP-Regular.ttf";

pub fn resolve_assets_base(project_root: &Path) -> PathBuf {
    if let Ok(exe_path) = std::env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
        && exe_dir.join("assets").is_dir()
    {
        return exe_dir.to_path_buf();
    }

    project_root.to_path_buf()
}
