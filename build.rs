use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=assets");
    if let Err(err) = copy_assets_to_profile_dir() {
        panic!("failed to copy assets next to executable: {err}");
    }
}

fn copy_assets_to_profile_dir() -> Result<(), String> {
    let project_root = env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .map_err(|err| format!("CARGO_MANIFEST_DIR is unavailable: {err}"))?;
    let profile = env::var("PROFILE").map_err(|err| format!("PROFILE is unavailable: {err}"))?;
    let out_dir = env::var("OUT_DIR")
        .map(PathBuf::from)
        .map_err(|err| format!("OUT_DIR is unavailable: {err}"))?;

    let source_assets_dir = project_root.join("assets");
    if !source_assets_dir.is_dir() {
        return Err(format!(
            "assets directory is missing: {}",
            source_assets_dir.display()
        ));
    }

    let profile_dir = resolve_profile_dir_from_out_dir(&out_dir, &profile).ok_or_else(|| {
        format!(
            "failed to resolve target profile directory from OUT_DIR: {}",
            out_dir.display()
        )
    })?;
    let destination_assets_dir = profile_dir.join("assets");
    copy_dir_recursive(&source_assets_dir, &destination_assets_dir)
        .map_err(|err| format!("copy failed: {err}"))?;
    Ok(())
}

fn resolve_profile_dir_from_out_dir(out_dir: &Path, profile: &str) -> Option<PathBuf> {
    out_dir
        .ancestors()
        .find(|ancestor| ancestor.file_name().and_then(|name| name.to_str()) == Some(profile))
        .map(Path::to_path_buf)
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());

        if source_path.is_dir() {
            copy_dir_recursive(&source_path, &destination_path)?;
            continue;
        }

        if source_path.is_file() {
            if let Some(parent) = destination_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source_path, &destination_path)?;
        }
    }
    Ok(())
}
