use crate::git;
use crate::models::{CommitInfo, RepoSnapshot};
use crate::paths;
use serde::Serialize;
use std::path::Path;

const OMITTED_TEXT: &str = "(省略: 200KB制限超過のため変更ファイル一覧のみ出力)";

#[derive(Debug, Clone)]
pub struct MaterialOutput {
    pub text: String,
    pub json: String,
    pub omitted_due_to_size: bool,
}

#[derive(Debug, Clone, Serialize)]
struct MinimalMaterial {
    repo_path: String,
    current_branch: String,
    head_full_id: String,
    head_message: String,
    head_datetime: String,
    target_full_id: String,
    dirty: bool,
    status_porcelain: String,
    recent_commits: Vec<CommitInfo>,
}

#[derive(Debug, Clone, Serialize)]
struct DetailedMaterial {
    #[serde(flatten)]
    minimal: MinimalMaterial,
    git_status: String,
    git_diff_unstaged: String,
    git_diff_staged: String,
    git_diff_target_to_head: String,
    omitted_due_to_size: bool,
    changed_files: Vec<String>,
    changed_file_count: usize,
}

pub fn generate_minimal(
    snapshot: &RepoSnapshot,
    target_full_id: &str,
) -> Result<MaterialOutput, String> {
    let minimal = build_minimal(snapshot, target_full_id);
    let text = render_minimal_text(&minimal);
    let json = serde_json::to_string_pretty(&minimal)
        .map_err(|err| format!("json serialize failed: {err}"))?;

    Ok(MaterialOutput {
        text,
        json,
        omitted_due_to_size: false,
    })
}

pub fn generate_detailed(
    repo_path: &Path,
    snapshot: &RepoSnapshot,
    target_full_id: &str,
) -> Result<MaterialOutput, String> {
    let minimal = build_minimal(snapshot, target_full_id);
    let git_data = git::collect_detailed_git_data(repo_path, target_full_id)?;

    let full = DetailedMaterial {
        minimal: minimal.clone(),
        git_status: git_data.status_full,
        git_diff_unstaged: git_data.diff_unstaged,
        git_diff_staged: git_data.diff_staged,
        git_diff_target_to_head: git_data.diff_target_to_head,
        omitted_due_to_size: false,
        changed_file_count: git_data.changed_files.len(),
        changed_files: git_data.changed_files.clone(),
    };

    let (full_text, full_json) = render_detailed_pair(&full)?;
    if within_limit(&full_text) && within_limit(&full_json) {
        return Ok(MaterialOutput {
            text: full_text,
            json: full_json,
            omitted_due_to_size: false,
        });
    }

    let mut truncated = DetailedMaterial {
        minimal,
        git_status: OMITTED_TEXT.to_string(),
        git_diff_unstaged: OMITTED_TEXT.to_string(),
        git_diff_staged: OMITTED_TEXT.to_string(),
        git_diff_target_to_head: OMITTED_TEXT.to_string(),
        omitted_due_to_size: true,
        changed_file_count: git_data.changed_files.len(),
        changed_files: git_data.changed_files,
    };

    fit_truncated_output(&mut truncated)
}

fn fit_truncated_output(material: &mut DetailedMaterial) -> Result<MaterialOutput, String> {
    let total_changed = material.changed_file_count;
    let mut visible_files = material.changed_files.clone();

    loop {
        material.changed_files = visible_files.clone();
        let omitted_files = total_changed.saturating_sub(material.changed_files.len());
        if omitted_files > 0 {
            material
                .changed_files
                .push(format!("... {omitted_files} files omitted"));
        }

        let (text, json) = render_detailed_pair(material)?;
        if within_limit(&text) && within_limit(&json) {
            return Ok(MaterialOutput {
                text,
                json,
                omitted_due_to_size: true,
            });
        }

        if visible_files.is_empty() {
            break;
        }
        visible_files.pop();
    }

    material.changed_files = vec![format!("{total_changed} files changed (list omitted)")];
    let (text, json) = render_detailed_pair(material)?;
    Ok(MaterialOutput {
        text,
        json,
        omitted_due_to_size: true,
    })
}

fn build_minimal(snapshot: &RepoSnapshot, target_full_id: &str) -> MinimalMaterial {
    MinimalMaterial {
        repo_path: snapshot.repo_path.to_string_lossy().to_string(),
        current_branch: snapshot.current_branch.clone(),
        head_full_id: snapshot.head_full_id.clone(),
        head_message: snapshot.head_message.clone(),
        head_datetime: snapshot.head_datetime.clone(),
        target_full_id: target_full_id.to_string(),
        dirty: snapshot.dirty,
        status_porcelain: snapshot.status_porcelain.clone(),
        recent_commits: snapshot.recent_commits.clone(),
    }
}

fn render_detailed_pair(material: &DetailedMaterial) -> Result<(String, String), String> {
    let text = render_detailed_text(material);
    let json = serde_json::to_string_pretty(material)
        .map_err(|err| format!("json serialize failed: {err}"))?;
    Ok((text, json))
}

fn render_minimal_text(material: &MinimalMaterial) -> String {
    let mut lines = vec![
        format!("repo_path: {}", material.repo_path),
        format!("current_branch: {}", material.current_branch),
        format!("head_full_id: {}", material.head_full_id),
        format!("head_message: {}", material.head_message),
        format!("head_datetime: {}", material.head_datetime),
        format!("target_full_id: {}", material.target_full_id),
        format!("dirty: {}", material.dirty),
        "status_porcelain:".to_string(),
        material.status_porcelain.clone(),
        "recent_commits:".to_string(),
    ];

    for commit in &material.recent_commits {
        lines.push(format!(
            "- {} | {} | {} | {}",
            commit.datetime, commit.short_id, commit.message, commit.full_id
        ));
    }

    lines.join("\n")
}

fn render_detailed_text(material: &DetailedMaterial) -> String {
    let mut lines = vec![
        render_minimal_text(&material.minimal),
        format!("omitted_due_to_size: {}", material.omitted_due_to_size),
        format!("changed_file_count: {}", material.changed_file_count),
        "changed_files:".to_string(),
    ];

    if material.changed_files.is_empty() {
        lines.push("(none)".to_string());
    } else {
        for file in &material.changed_files {
            lines.push(format!("- {file}"));
        }
    }

    lines.push("git_status:".to_string());
    lines.push(material.git_status.clone());
    lines.push("git_diff_unstaged:".to_string());
    lines.push(material.git_diff_unstaged.clone());
    lines.push("git_diff_staged:".to_string());
    lines.push(material.git_diff_staged.clone());
    lines.push("git_diff_target_to_head:".to_string());
    lines.push(material.git_diff_target_to_head.clone());

    lines.join("\n")
}

fn within_limit(content: &str) -> bool {
    utf8_len(content) <= paths::DETAILED_OUTPUT_LIMIT_BYTES
}

fn utf8_len(content: &str) -> usize {
    content.len()
}

#[cfg(test)]
mod tests {
    use super::utf8_len;

    #[test]
    fn utf8_len_counts_multibyte_bytes() {
        assert_eq!(utf8_len("abc"), 3);
        assert_eq!(utf8_len("日本語"), 9);
    }
}
