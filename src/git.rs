use crate::models::{CommitInfo, RepoCandidate, RepoSnapshot, ScanScope};
use chrono::{Datelike, FixedOffset, TimeZone, Utc};
use std::collections::HashSet;
use std::fs;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

const DATETIME_FETCH_FAILED: &str = "(取得失敗)";
const JST_OFFSET_SECONDS: i32 = 9 * 60 * 60;
const MAIN_BRANCH_NAME: &str = "main";
const RECORD_SEPARATOR: u8 = 0x00;
const FIELD_SEPARATOR: u8 = 0x1f;

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
            get_last_commit_datetime(&dir).unwrap_or_else(|_| DATETIME_FETCH_FAILED.to_string());

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
    let (head_full_id, head_message, head_message_full, head_datetime) = get_head_info(repo_path)?;
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
        head_message_full,
        head_datetime,
        dirty,
        status_porcelain,
        recent_commits,
        consecutive_update_failures: 0,
    })
}

pub fn get_commit_message_full(repo_path: &Path, commit_id: &str) -> Result<String, String> {
    let normalized_id = commit_id.trim();
    if normalized_id.is_empty() {
        return Err("commit id is empty".to_string());
    }

    run_git(repo_path, &["show", "-s", "--format=%B", normalized_id])
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
    let timestamp_raw = run_git(repo_path, &["log", "-1", "--pretty=format:%ct"])?;
    Ok(format_git_timestamp_or_fallback(&timestamp_raw))
}

fn get_head_info(repo_path: &Path) -> Result<(String, String, String, String), String> {
    let output = run_git_bytes(
        repo_path,
        &["log", "-1", "--pretty=format:%H%x1f%s%x1f%ct%x1f%B%x00"],
    )?;

    let mut records = parse_records(&output, 4)?;
    let head_record = records
        .pop()
        .ok_or_else(|| "HEAD log record is empty".to_string())?;
    let [
        head_full_id,
        head_message,
        head_timestamp_raw,
        head_message_full,
    ] = head_record
        .try_into()
        .map_err(|_| "HEAD log record field conversion failed".to_string())?;

    if head_full_id.trim().is_empty() {
        return Err("HEAD full id is empty".to_string());
    }
    let head_datetime = format_git_timestamp_or_fallback(&head_timestamp_raw);

    Ok((
        head_full_id,
        head_message,
        trim_git_full_message_tail(head_message_full),
        head_datetime,
    ))
}

fn list_recent_commits(repo_path: &Path, max_count: usize) -> Result<Vec<CommitInfo>, String> {
    let max_count_arg = max_count.to_string();
    let output = run_git_bytes(
        repo_path,
        &[
            "log",
            "--all",
            "-n",
            max_count_arg.as_str(),
            "--pretty=format:%H%x1f%h%x1f%ct%x1f%s%x1f%B%x00",
        ],
    )?;

    let main_head_commit_id = get_main_branch_head_commit_id(repo_path)?;
    let main_commit_ids = collect_main_commit_ids(repo_path)?;
    let records = parse_records(&output, 5)?;

    let mut commits = Vec::new();
    for record in records {
        let [full_id, short_id, timestamp_raw, subject, body_full] = record
            .try_into()
            .map_err(|_| "commit log record field conversion failed".to_string())?;
        if full_id.trim().is_empty() {
            continue;
        }
        commits.push(CommitInfo {
            scope_mark: build_scope_mark(&main_head_commit_id, &full_id),
            in_main_history: is_main_history_commit(&main_commit_ids, &full_id),
            datetime: format_git_timestamp_or_fallback(&timestamp_raw),
            short_id,
            subject,
            body_full: trim_git_full_message_tail(body_full),
            full_id,
        });
    }

    Ok(commits)
}

fn parse_records(output: &[u8], expected_fields: usize) -> Result<Vec<Vec<String>>, String> {
    let mut records = Vec::new();
    for record in output.split(|byte| *byte == RECORD_SEPARATOR) {
        if record.is_empty() {
            continue;
        }
        let fields = record
            .split(|byte| *byte == FIELD_SEPARATOR)
            .map(|field| String::from_utf8_lossy(field).to_string())
            .collect::<Vec<_>>();
        if fields.len() != expected_fields {
            return Err(format!(
                "unexpected git log field count: expected={expected_fields} actual={} raw='{}'",
                fields.len(),
                String::from_utf8_lossy(record)
            ));
        }
        records.push(fields);
    }
    Ok(records)
}

fn trim_git_full_message_tail(full_message: String) -> String {
    full_message.trim_end_matches('\n').to_string()
}

fn format_git_timestamp_or_fallback(timestamp_raw: &str) -> String {
    format_git_timestamp_for_display(timestamp_raw)
        .unwrap_or_else(|| DATETIME_FETCH_FAILED.to_string())
}

fn format_git_timestamp_for_display(timestamp_raw: &str) -> Option<String> {
    let jst = FixedOffset::east_opt(JST_OFFSET_SECONDS)?;
    let now_jst = Utc::now().with_timezone(&jst);
    format_git_timestamp_for_display_with_now(timestamp_raw, now_jst)
}

fn format_git_timestamp_for_display_with_now(
    timestamp_raw: &str,
    now_jst: chrono::DateTime<FixedOffset>,
) -> Option<String> {
    let timestamp = timestamp_raw.trim().parse::<i64>().ok()?;
    let commit_jst = Utc
        .timestamp_opt(timestamp, 0)
        .single()?
        .with_timezone(now_jst.offset());
    let format = if commit_jst.year() == now_jst.year() {
        "%m/%d %H:%M"
    } else {
        "%y/%m/%d %H:%M"
    };
    Some(commit_jst.format(format).to_string())
}

fn get_main_branch_head_commit_id(repo_path: &Path) -> Result<Option<String>, String> {
    let branch_ref = format!("refs/heads/{MAIN_BRANCH_NAME}");
    let output = Command::new("git")
        .args(["rev-parse", "--verify", branch_ref.as_str()])
        .current_dir(repo_path)
        .output()
        .map_err(|err| format!("failed to launch git (rev-parse): {err}"))?;

    match output.status.code() {
        Some(0) => {
            let commit_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if commit_id.is_empty() {
                Ok(None)
            } else {
                Ok(Some(commit_id))
            }
        }
        Some(1) => Ok(None),
        _ => Err(format!(
            "git rev-parse failed: status={} stderr='{}'",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
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

fn is_main_history_commit(main_commit_ids: &Option<HashSet<String>>, commit_full_id: &str) -> bool {
    let normalized_commit_full_id = commit_full_id.trim();
    main_commit_ids
        .as_ref()
        .map(|ids| ids.contains(normalized_commit_full_id))
        .unwrap_or(false)
}

fn build_scope_mark(main_head_commit_id: &Option<String>, commit_full_id: &str) -> String {
    let normalized_commit_full_id = commit_full_id.trim();
    if main_head_commit_id
        .as_ref()
        .map(|main_id| main_id.trim() == normalized_commit_full_id)
        .unwrap_or(false)
    {
        "M".to_string()
    } else {
        String::new()
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
    let stdout = run_git_bytes(repo_path, args)?;
    Ok(String::from_utf8_lossy(&stdout)
        .trim_end_matches(['\r', '\n'])
        .to_string())
}

fn run_git_bytes(repo_path: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
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

    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::{
        JST_OFFSET_SECONDS, build_scope_mark, format_git_timestamp_for_display_with_now,
        parse_records, trim_git_full_message_tail,
    };
    use chrono::{FixedOffset, TimeZone};

    #[test]
    fn build_scope_mark_returns_m_when_commit_is_main_branch_head() {
        let main_head = Some("abc".to_string());
        assert_eq!(build_scope_mark(&main_head, "abc"), "M");
    }

    #[test]
    fn build_scope_mark_returns_empty_when_commit_is_not_main_branch_head() {
        let main_head = Some("abc".to_string());
        assert_eq!(build_scope_mark(&main_head, "def"), "");
    }

    #[test]
    fn build_scope_mark_handles_missing_main_branch_as_empty() {
        assert_eq!(build_scope_mark(&None, "def"), "");
    }

    #[test]
    fn parse_records_reads_nul_and_us_separated_records() {
        let bytes = b"full1\x1fshort1\x1f2026/01/01 10:00\x1fsubject1\x1fsubject1\nbody1\n\x00full2\x1fshort2\x1f2026/01/02 11:00\x1fsubject2\x1fsubject2\x00";
        let records = parse_records(bytes, 5).expect("records should parse");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0][0], "full1");
        assert_eq!(records[0][3], "subject1");
        assert_eq!(records[0][4], "subject1\nbody1\n");
        assert_eq!(records[1][3], "subject2");
        assert_eq!(records[1][4], "subject2");
    }

    #[test]
    fn trim_git_full_message_tail_removes_only_trailing_newline() {
        let trimmed = trim_git_full_message_tail("subject\n\nbody\n".to_string());
        assert_eq!(trimmed, "subject\n\nbody");
        let unchanged = trim_git_full_message_tail("subject\n\nbody".to_string());
        assert_eq!(unchanged, "subject\n\nbody");
    }

    #[test]
    fn format_git_timestamp_for_display_uses_mmdd_when_same_year_in_jst() {
        let jst = FixedOffset::east_opt(JST_OFFSET_SECONDS).expect("JST offset should exist");
        let now_jst = jst
            .with_ymd_and_hms(2026, 2, 21, 16, 0, 0)
            .single()
            .expect("valid datetime");
        let commit_jst = jst
            .with_ymd_and_hms(2026, 1, 1, 9, 5, 0)
            .single()
            .expect("valid datetime");
        let formatted =
            format_git_timestamp_for_display_with_now(&commit_jst.timestamp().to_string(), now_jst);
        assert_eq!(formatted, Some("01/01 09:05".to_string()));
    }

    #[test]
    fn format_git_timestamp_for_display_uses_yy_when_year_differs_in_jst() {
        let jst = FixedOffset::east_opt(JST_OFFSET_SECONDS).expect("JST offset should exist");
        let now_jst = jst
            .with_ymd_and_hms(2026, 2, 21, 16, 0, 0)
            .single()
            .expect("valid datetime");
        let commit_jst = jst
            .with_ymd_and_hms(2025, 12, 31, 23, 59, 0)
            .single()
            .expect("valid datetime");
        let formatted =
            format_git_timestamp_for_display_with_now(&commit_jst.timestamp().to_string(), now_jst);
        assert_eq!(formatted, Some("25/12/31 23:59".to_string()));
    }

    #[test]
    fn format_git_timestamp_for_display_returns_none_for_invalid_timestamp() {
        let jst = FixedOffset::east_opt(JST_OFFSET_SECONDS).expect("JST offset should exist");
        let now_jst = jst
            .with_ymd_and_hms(2026, 2, 21, 16, 0, 0)
            .single()
            .expect("valid datetime");
        let formatted = format_git_timestamp_for_display_with_now("not-a-number", now_jst);
        assert_eq!(formatted, None);
    }
}
