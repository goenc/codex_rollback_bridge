use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScanScope {
    #[default]
    Direct,
    Depth2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub schema_version: u32,
    pub root_folder_path: Option<String>,
    pub scan_scope: ScanScope,
    pub update_interval_sec: u64,
    pub selected_repo_path: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            root_folder_path: None,
            scan_scope: ScanScope::Direct,
            update_interval_sec: 3,
            selected_repo_path: None,
        }
    }
}

impl Settings {
    pub fn normalize(&mut self) {
        if !(1..=3600).contains(&self.update_interval_sec) {
            self.update_interval_sec = 3;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppStatus {
    ProjectUnselected,
    Selected,
    Updating,
    Error,
}

#[derive(Debug, Clone)]
pub struct RepoCandidate {
    pub project_name: String,
    pub path: PathBuf,
    pub last_commit_datetime: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkState {
    Clean,
    Dirty,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitOperation {
    Rebase,
    Merge,
    CherryPick,
    Revert,
    Bisect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitInfo {
    pub scope_mark: String,
    #[serde(default)]
    pub in_main_history: bool,
    pub datetime: String,
    pub short_id: String,
    #[serde(default, alias = "message")]
    pub subject: String,
    #[serde(default)]
    pub body_full: String,
    pub full_id: String,
}

#[derive(Debug, Clone)]
pub struct RepoSnapshot {
    pub repo_path: PathBuf,
    pub current_branch: String,
    pub head_full_id: String,
    pub head_message: String,
    pub head_message_full: String,
    pub head_datetime: String,
    pub work_state: WorkState,
    pub operation: Option<GitOperation>,
    pub dirty: bool,
    pub has_runtime_changes: bool,
    pub has_source_changes: bool,
    pub status_porcelain: String,
    pub recent_commits: Vec<CommitInfo>,
    pub consecutive_update_failures: u32,
}
