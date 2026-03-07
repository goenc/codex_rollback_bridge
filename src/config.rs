use crate::models::{ScanScope, Settings};
use crate::paths;
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

const EXTERNAL_SELECTED_REPO_FILE: &str = "selected_repo_path.txt";

pub struct LoadSettingsResult {
    pub settings: Settings,
    pub logs: Vec<String>,
}

#[derive(Default)]
struct PartialSettings {
    schema_version: Option<u32>,
    root_folder_path: Option<Option<String>>,
    scan_scope: Option<ScanScope>,
    update_interval_sec: Option<u64>,
    selected_repo_path: Option<Option<String>>,
}

pub fn config_contract_line() -> String {
    let runtime_pattern = format!(
        "%AppData%\\{}\\*.override{}",
        paths::APP_NAME,
        paths::JSON_EXTENSION
    );
    format!(
        "config_contract: default={}/{}.default{} runtime={runtime_pattern}",
        paths::CONFIG_DIR_RELATIVE.replace('\\', "/"),
        "*",
        paths::JSON_EXTENSION
    )
}

pub fn default_settings_path(project_root: &Path) -> PathBuf {
    project_root
        .join(paths::CONFIG_DIR_RELATIVE)
        .join(paths::SETTINGS_DEFAULT_FILE)
}

pub fn runtime_override_path() -> Result<PathBuf, String> {
    let base_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".config"))
        })
        .ok_or_else(|| "APPDATA and HOME are unavailable".to_string())?;

    Ok(base_dir
        .join(paths::APP_NAME)
        .join(paths::SETTINGS_OVERRIDE_FILE))
}

pub fn external_selected_repo_path_file() -> Result<PathBuf, String> {
    let mut path = runtime_override_path()?;
    path.set_file_name(EXTERNAL_SELECTED_REPO_FILE);
    Ok(path)
}

pub fn load_external_selected_repo_path() -> Result<Option<String>, String> {
    let path = external_selected_repo_path_file()?;
    if !path.exists() {
        return Ok(None);
    }
    if !path.is_file() {
        return Err(format!("external selected repo path is not a file: {}", path.display()));
    }

    let contents = fs::read_to_string(&path)
        .map_err(|err| format!("external selected repo path read failed '{}': {err}", path.display()))?;
    Ok(normalize_external_selected_repo_path(&contents))
}

pub fn save_external_selected_repo_path(selected_repo_path: Option<&Path>) -> Result<PathBuf, String> {
    let path = external_selected_repo_path_file()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create external selected repo directory: {err}"))?;
    }

    let contents = selected_repo_path
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    fs::write(&path, contents)
        .map_err(|err| format!("external selected repo path write failed '{}': {err}", path.display()))?;
    Ok(path)
}

pub fn load_settings(project_root: &Path) -> LoadSettingsResult {
    let mut logs = Vec::new();
    let mut settings = Settings::default();

    let default_path = default_settings_path(project_root);
    let default_source = format!("default({})", default_path.display());
    let default_partial = parse_partial_settings(&default_path, &default_source, true, &mut logs);
    apply_partial(&mut settings, default_partial);

    match runtime_override_path() {
        Ok(override_path) => {
            if override_path.exists() {
                let override_source = format!("override({})", override_path.display());
                let override_partial =
                    parse_partial_settings(&override_path, &override_source, false, &mut logs);
                apply_partial(&mut settings, override_partial);
            }
        }
        Err(err) => logs.push(format!("override path unavailable: {err}")),
    }

    settings.normalize();
    LoadSettingsResult { settings, logs }
}

pub fn save_override(settings: &Settings) -> Result<PathBuf, String> {
    let override_path = runtime_override_path()?;
    if let Some(parent) = override_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create override directory: {err}"))?;
    }

    let json = serde_json::to_string_pretty(settings)
        .map_err(|err| format!("json serialize failed: {err}"))?;
    fs::write(&override_path, json).map_err(|err| format!("override write failed: {err}"))?;
    Ok(override_path)
}

fn parse_partial_settings(
    path: &Path,
    source: &str,
    strict_required: bool,
    logs: &mut Vec<String>,
) -> PartialSettings {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) => {
            if strict_required {
                logs.push(format!("{source}: read failed ({err})"));
            }
            return PartialSettings::default();
        }
    };

    let value: Value = match serde_json::from_str(&contents) {
        Ok(value) => value,
        Err(err) => {
            logs.push(format!("{source}: json parse failed ({err})"));
            return PartialSettings::default();
        }
    };

    let Some(map) = value.as_object() else {
        logs.push(format!("{source}: root must be a JSON object"));
        return PartialSettings::default();
    };

    extract_partial_from_map(map, source, strict_required, logs)
}

fn extract_partial_from_map(
    map: &Map<String, Value>,
    source: &str,
    strict_required: bool,
    logs: &mut Vec<String>,
) -> PartialSettings {
    PartialSettings {
        schema_version: read_u32(map, "schema_version", source, strict_required, logs),
        root_folder_path: read_nullable_string(
            map,
            "root_folder_path",
            source,
            strict_required,
            logs,
        ),
        scan_scope: read_scan_scope(map, "scan_scope", source, strict_required, logs),
        update_interval_sec: read_update_interval(
            map,
            "update_interval_sec",
            source,
            strict_required,
            logs,
        ),
        selected_repo_path: read_nullable_string(
            map,
            "selected_repo_path",
            source,
            strict_required,
            logs,
        ),
    }
}

fn apply_partial(settings: &mut Settings, partial: PartialSettings) {
    if let Some(schema_version) = partial.schema_version {
        settings.schema_version = schema_version;
    }
    if let Some(root_folder_path) = partial.root_folder_path {
        settings.root_folder_path = root_folder_path;
    }
    if let Some(scan_scope) = partial.scan_scope {
        settings.scan_scope = scan_scope;
    }
    if let Some(update_interval_sec) = partial.update_interval_sec {
        settings.update_interval_sec = update_interval_sec;
    }
    if let Some(selected_repo_path) = partial.selected_repo_path {
        settings.selected_repo_path = selected_repo_path;
    }
}

fn read_u32(
    map: &Map<String, Value>,
    key: &str,
    source: &str,
    strict_required: bool,
    logs: &mut Vec<String>,
) -> Option<u32> {
    let Some(value) = map.get(key) else {
        if strict_required {
            logs.push(format!("{source}: missing key '{key}'"));
        }
        return None;
    };

    let Some(raw) = value.as_u64() else {
        logs.push(format!("{source}: key '{key}' must be integer"));
        return None;
    };

    match u32::try_from(raw) {
        Ok(value) => Some(value),
        Err(_) => {
            logs.push(format!("{source}: key '{key}' is out of range"));
            None
        }
    }
}

fn read_nullable_string(
    map: &Map<String, Value>,
    key: &str,
    source: &str,
    strict_required: bool,
    logs: &mut Vec<String>,
) -> Option<Option<String>> {
    let Some(value) = map.get(key) else {
        if strict_required {
            logs.push(format!("{source}: missing key '{key}'"));
        }
        return None;
    };

    if value.is_null() {
        return Some(None);
    }

    match value.as_str() {
        Some(text) => Some(Some(text.to_string())),
        None => {
            logs.push(format!("{source}: key '{key}' must be string or null"));
            None
        }
    }
}

fn read_scan_scope(
    map: &Map<String, Value>,
    key: &str,
    source: &str,
    strict_required: bool,
    logs: &mut Vec<String>,
) -> Option<ScanScope> {
    let Some(value) = map.get(key) else {
        if strict_required {
            logs.push(format!("{source}: missing key '{key}'"));
        }
        return None;
    };

    let Some(raw) = value.as_str() else {
        logs.push(format!("{source}: key '{key}' must be string"));
        return None;
    };

    match raw {
        "direct" => Some(ScanScope::Direct),
        "depth2" => Some(ScanScope::Depth2),
        _ => {
            logs.push(format!("{source}: key '{key}' has invalid value '{raw}'"));
            None
        }
    }
}

fn read_update_interval(
    map: &Map<String, Value>,
    key: &str,
    source: &str,
    strict_required: bool,
    logs: &mut Vec<String>,
) -> Option<u64> {
    let Some(value) = map.get(key) else {
        if strict_required {
            logs.push(format!("{source}: missing key '{key}'"));
        }
        return None;
    };

    let Some(raw) = value.as_u64() else {
        logs.push(format!("{source}: key '{key}' must be integer"));
        return None;
    };

    if !(1..=3600).contains(&raw) {
        logs.push(format!(
            "{source}: key '{key}' must be in range 1..=3600 (got {raw})"
        ));
        return None;
    }

    Some(raw)
}

fn normalize_external_selected_repo_path(contents: &str) -> Option<String> {
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_external_selected_repo_path;

    #[test]
    fn normalize_external_selected_repo_path_returns_none_for_empty_input() {
        assert_eq!(normalize_external_selected_repo_path(""), None);
        assert_eq!(normalize_external_selected_repo_path(" \n\t "), None);
    }

    #[test]
    fn normalize_external_selected_repo_path_trims_whitespace() {
        assert_eq!(
            normalize_external_selected_repo_path(" C:\\repo\\sample \r\n"),
            Some("C:\\repo\\sample".to_string())
        );
    }
}
