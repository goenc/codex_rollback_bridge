use crate::models::RepoSnapshot;

fn context_header(snapshot: &RepoSnapshot) -> String {
    let _ = snapshot.head_message_full.as_str();
    let status_porcelain = if snapshot.status_porcelain.trim().is_empty() {
        "(clean)".to_string()
    } else {
        snapshot
            .status_porcelain
            .lines()
            .collect::<Vec<_>>()
            .join(" | ")
    };

    format!(
        "# repo: {repo_path}\n\
# branch: {current_branch}\n\
# head: {head_full_id}\n\
# head_message: {head_message}\n\
# head_datetime: {head_datetime}\n\
# dirty: {dirty}\n\
# status_porcelain: {status_porcelain}\n",
        repo_path = snapshot.repo_path.to_string_lossy(),
        current_branch = snapshot.current_branch,
        head_full_id = snapshot.head_full_id,
        head_message = snapshot.head_message,
        head_datetime = snapshot.head_datetime,
        dirty = snapshot.dirty,
        status_porcelain = status_porcelain
    )
}

pub fn build_previous_head_rollback_command(snapshot: &RepoSnapshot) -> String {
    let repo_path = snapshot.repo_path.to_string_lossy();
    let current_branch = snapshot.current_branch.trim();
    let header = context_header(snapshot);
    format!(
        "{header}git -C \"{repo_path}\" rev-parse --verify HEAD~1\n\
git -C \"{repo_path}\" rev-parse --abbrev-ref HEAD\n\
git -C \"{repo_path}\" reset --hard HEAD~1\n\
git -C \"{repo_path}\" push --force-with-lease origin {current_branch}\n\
git -C \"{repo_path}\" status --short\n\
git -C \"{repo_path}\" log --oneline -n 3"
    )
}
