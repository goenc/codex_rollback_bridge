use crate::models::RepoSnapshot;

pub fn build_codex_instruction(snapshot: &RepoSnapshot) -> String {
    format!(
        "Codexへ: 次のリポジトリをロールバックする最小手順を提示してください。\n\
repo: {repo_path}\n\
branch: {current_branch}\n\
rollback_target: HEAD\n\
head: {head_full_id}\n\
head_message: {head_message}\n\
head_datetime: {head_datetime}\n\
dirty: {dirty}\n\
status:\n\
{status_porcelain}",
        repo_path = snapshot.repo_path.to_string_lossy(),
        current_branch = snapshot.current_branch,
        head_full_id = snapshot.head_full_id,
        head_message = snapshot.head_message,
        head_datetime = snapshot.head_datetime,
        dirty = snapshot.dirty,
        status_porcelain = snapshot.status_porcelain
    )
}
