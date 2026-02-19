use crate::models::{CommitInfo, RepoCandidate, RepoSnapshot, ScanScope};
use std::collections::HashSet;
use std::fs;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

const GIT_DATE_FORMAT_ARG: &str = "--date=format:%Y/%m/%d %H:%M";
const MAIN_BRANCH_NAME: &str = "main";

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

        let requirement_headline =
            read_requirement_headline(&dir).unwrap_or_else(|| "(要件定義書なし)".to_string());
        let last_commit_datetime =
            get_last_commit_datetime(&dir).unwrap_or_else(|_| "(取得失敗)".to_string());

        repositories.push(RepoCandidate {
            requirement_headline,
            path: dir,
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
        &["log", "-1", GIT_DATE_FORMAT_ARG, "--pretty=format:%cd"],
    )
}

fn get_head_info(repo_path: &Path) -> Result<(String, String, String), String> {
    let output = run_git(
        repo_path,
        &[
            "log",
            "-1",
            GIT_DATE_FORMAT_ARG,
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
            "--all",
            "-n",
            max_count_arg.as_str(),
            GIT_DATE_FORMAT_ARG,
            "--pretty=format:%H%x1f%h%x1f%cd%x1f%s",
        ],
    )?;

    let main_commit_ids = collect_main_commit_ids(repo_path)?;

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
            scope_mark: classify_scope_mark(&main_commit_ids, &full_id).to_string(),
            datetime,
            short_id,
            message,
            full_id,
        });
    }

    Ok(commits)
}

fn collect_main_commit_ids(repo_path: &Path) -> Result<Option<HashSet<String>>, String> {
    if !local_branch_exists(repo_path, MAIN_BRANCH_NAME)? {
        return Ok(None);
    }

    let rev_list = run_git(repo_path, &["rev-list", MAIN_BRANCH_NAME])?;
    let ids = rev_list
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect::<HashSet<_>>();

    Ok(Some(ids))
}

fn local_branch_exists(repo_path: &Path, branch_name: &str) -> Result<bool, String> {
    let branch_ref = format!("refs/heads/{branch_name}");
    let output = Command::new("git")
        .args(["show-ref", "--verify", "--quiet", branch_ref.as_str()])
        .current_dir(repo_path)
        .output()
        .map_err(|err| format!("failed to launch git (show-ref): {err}"))?;

    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!(
            "git show-ref failed: status={} stderr='{}'",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

fn classify_scope_mark(main_commit_ids: &Option<HashSet<String>>, full_id: &str) -> &'static str {
    if main_commit_ids
        .as_ref()
        .map(|ids| ids.contains(full_id))
        .unwrap_or(false)
    {
        "M"
    } else {
        "P"
    }
}

fn read_requirement_headline(repo_path: &Path) -> Option<String> {
    let mut definition_files: Vec<PathBuf> = fs::read_dir(repo_path)
        .ok()?
        .filter_map(|entry| entry.ok().map(|item| item.path()))
        .filter(|path| path.is_file())
        .filter(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy())
                .map(|name| name.starts_with("要件定義_") && name.ends_with(".md"))
                .unwrap_or(false)
        })
        .collect();

    definition_files.sort();
    for path in definition_files {
        let Ok(file) = File::open(&path) else {
            continue;
        };
        let mut reader = BufReader::new(file);
        let mut first_line = String::new();
        let Ok(read_size) = reader.read_line(&mut first_line) else {
            continue;
        };
        if read_size == 0 {
            continue;
        }
        let trimmed = first_line.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    None
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

#[cfg(test)]
mod tests {
    use super::classify_scope_mark;
    use std::collections::HashSet;

    #[test]
    fn classify_scope_mark_returns_m_when_commit_is_in_main_history() {
        let mut ids = HashSet::new();
        ids.insert("abc".to_string());
        let main_ids = Some(ids);
        assert_eq!(classify_scope_mark(&main_ids, "abc"), "M");
    }

    #[test]
    fn classify_scope_mark_returns_p_when_commit_is_not_in_main_history() {
        let mut ids = HashSet::new();
        ids.insert("abc".to_string());
        let main_ids = Some(ids);
        assert_eq!(classify_scope_mark(&main_ids, "def"), "P");
        assert_eq!(classify_scope_mark(&None, "def"), "P");
    }
}
