use crate::models::RepoSnapshot;

pub fn build_codex_instruction(
    snapshot: &RepoSnapshot,
    target_full_id: &str,
    target_message: &str,
) -> String {
    format!(
        "あなたはGit操作ができる開発補助AIです。\n\
対象リポジトリ: {repo_path}\n\
現在ブランチ: {current_branch}\n\
現在HEAD: {head_full_id}\n\
HEADメッセージ: {head_message}\n\
HEAD日時: {head_datetime}\n\
\n\
ターゲットコミット: {target_full_id}\n\
ターゲットメッセージ: {target_message}\n\
\n\
dirty: {dirty}\n\
status_porcelain:\n\
{status_porcelain}\n\
\n\
やること:\n\
1) dirty=true の場合、まず安全策を提案し、ユーザーに選ばせる（例: stash/退避/コミット）\n\
2) 目的確認: 「履歴を戻す(reset系)」か「打ち消し(revert系)」かを確認してから提案する\n\
3) 実行コマンドは必ず git -C \"{repo_path}\" 形式で提示する\n\
4) 最後に、復帰後の確認コマンド（git status / git log など）も提示する",
        repo_path = snapshot.repo_path.to_string_lossy(),
        current_branch = snapshot.current_branch,
        head_full_id = snapshot.head_full_id,
        head_message = snapshot.head_message,
        head_datetime = snapshot.head_datetime,
        target_full_id = target_full_id,
        target_message = target_message,
        dirty = snapshot.dirty,
        status_porcelain = snapshot.status_porcelain
    )
}
