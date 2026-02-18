use crate::models::{CommitInfo, RepoCandidate, RepoSnapshot, ScanScope};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct DetailedGitData {
    pub status_full: String,
    pub diff_unstaged: String,
    pub diff_staged: String,
    pub diff_target_to_head: String,
    pub changed_files: Vec<String>,
}

pub fn git_exists() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

pub fn is_git_repo(path: &Path) -> bool {
    path.join(".git").is_dir()
}

pub fn scan_repositories(root: &Path, scope: ScanScope) -> Result<Vec<RepoCandidate>, String> {
    if !root.is_dir() {
        return Err(format!("scan root is not a directory: {}", root.display()));
    }

    let scan_dirs = collect_scan_dirs(root, scope)?;
    let mut repositories = Vec::new();

    for dir in scan_dirs {
        if !is_git_repo(&dir) {
            continue;
        }

        let folder_name = dir
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| dir.to_string_lossy().to_string());
        let current_branch = get_current_branch(&dir).unwrap_or_else(|_| "(取得失敗)".to_string());
        let last_commit_datetime =
            get_last_commit_datetime(&dir).unwrap_or_else(|_| "(取得失敗)".to_string());

        repositories.push(RepoCandidate {
            folder_name,
            path: dir,
            current_branch,
            last_commit_datetime,
        });
    }

    repositories.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(repositories)
}

pub fn fetch_snapshot(
    repo_path: &Path,
    previous_head: &Option<String>,
    previous_commits: &[CommitInfo],
) -> Result<RepoSnapshot, String> {
    let current_branch = get_current_branch(repo_path)?;
    let (head_full_id, head_message, head_datetime) = get_head_info(repo_path)?;
    let status_porcelain = run_git(repo_path, &["status", "--porcelain"])?;
    let dirty = !status_porcelain.trim().is_empty();

    let head_changed = previous_head.as_deref() != Some(head_full_id.as_str());
    let recent_commits = if head_changed || previous_commits.is_empty() {
        list_recent_commits(repo_path, 50)?
    } else {
        previous_commits.to_vec()
    };

    Ok(RepoSnapshot {
        repo_path: repo_path.to_path_buf(),
        current_branch,
        head_full_id,
        head_message,
        head_datetime,
        dirty,
        status_porcelain,
        recent_commits,
        consecutive_update_failures: 0,
    })
}

pub fn collect_detailed_git_data(
    repo_path: &Path,
    target_full_id: &str,
) -> Result<DetailedGitData, String> {
    let status_full = run_git(repo_path, &["status"])?;
    let diff_unstaged = run_git(repo_path, &["diff"])?;
    let diff_staged = run_git(repo_path, &["diff", "--staged"])?;
    let range = format!("{target_full_id}..HEAD");
    let diff_target_to_head = run_git(repo_path, &["diff", range.as_str()])?;

    let changed_files = collect_changed_files(repo_path, range.as_str())?;

    Ok(DetailedGitData {
        status_full,
        diff_unstaged,
        diff_staged,
        diff_target_to_head,
        changed_files,
    })
}

fn collect_scan_dirs(root: &Path, scope: ScanScope) -> Result<Vec<PathBuf>, String> {
    let mut dirs = vec![root.to_path_buf()];

    for entry in fs::read_dir(root).map_err(|err| format!("read_dir failed: {err}"))? {
        let entry = entry.map_err(|err| format!("read_dir entry failed: {err}"))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        dirs.push(path.clone());
        if scope == ScanScope::Depth2 {
            for child_entry in
                fs::read_dir(&path).map_err(|err| format!("read_dir failed: {err}"))?
            {
                let child_entry =
                    child_entry.map_err(|err| format!("read_dir entry failed: {err}"))?;
                let child_path = child_entry.path();
                if child_path.is_dir() {
                    dirs.push(child_path);
                }
            }
        }
    }

    dirs.sort();
    dirs.dedup();
    Ok(dirs)
}

fn get_current_branch(repo_path: &Path) -> Result<String, String> {
    let branch = run_git(repo_path, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch.is_empty() {
        return Err("git branch output is empty".to_string());
    }
    Ok(branch)
}

fn get_last_commit_datetime(repo_path: &Path) -> Result<String, String> {
    run_git(
        repo_path,
        &["log", "-1", "--date=iso-strict", "--pretty=format:%cd"],
    )
}

fn get_head_info(repo_path: &Path) -> Result<(String, String, String), String> {
    let output = run_git(
        repo_path,
        &[
            "log",
            "-1",
            "--date=iso-strict",
            "--pretty=format:%H%n%s%n%cd",
        ],
    )?;

    let mut lines = output.lines();
    let head_full_id = lines.next().unwrap_or_default().trim().to_string();
    let head_message = lines.next().unwrap_or_default().trim().to_string();
    let head_datetime = lines.next().unwrap_or_default().trim().to_string();

    if head_full_id.is_empty() {
        return Err("HEAD full id is empty".to_string());
    }

    Ok((head_full_id, head_message, head_datetime))
}

fn list_recent_commits(repo_path: &Path, max_count: usize) -> Result<Vec<CommitInfo>, String> {
    let max_count_arg = max_count.to_string();
    let output = run_git(
        repo_path,
        &[
            "log",
            "-n",
            max_count_arg.as_str(),
            "--date=iso-strict",
            "--pretty=format:%H%x1f%h%x1f%cd%x1f%s",
        ],
    )?;

    let mut commits = Vec::new();
    for line in output.lines() {
        let mut parts = line.split('\u{001f}');
        let full_id = parts.next().unwrap_or_default().to_string();
        if full_id.is_empty() {
            continue;
        }
        let short_id = parts.next().unwrap_or_default().to_string();
        let datetime = parts.next().unwrap_or_default().to_string();
        let message = parts.next().unwrap_or_default().to_string();
        commits.push(CommitInfo {
            datetime,
            short_id,
            message,
            full_id,
        });
    }

    Ok(commits)
}

fn collect_changed_files(repo_path: &Path, target_range: &str) -> Result<Vec<String>, String> {
    let mut files = BTreeSet::new();

    for output in [
        run_git(repo_path, &["diff", "--name-only"])?,
        run_git(repo_path, &["diff", "--name-only", "--staged"])?,
        run_git(repo_path, &["diff", "--name-only", target_range])?,
    ] {
        for line in output.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                files.insert(trimmed.to_string());
            }
        }
    }

    Ok(files.into_iter().collect())
}

fn run_git(repo_path: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .output()
        .map_err(|err| format!("failed to launch git ({args:?}): {err}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "git command failed ({args:?}): status={} stderr='{}' stdout='{}'",
            output.status,
            stderr.trim(),
            stdout.trim()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(stdout.trim_end_matches(['\r', '\n']).to_string())
}
