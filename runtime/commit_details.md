日時: 2026-02-23 02:46:20 JST
対象: codex_rollback_bridge
summary: 修正時の調査切替ルールを変更容易な構成で実装した
code_changes:
・AGENTS.md の Implementation Phase に実行規則の参照を追加した
・skills に implementation_research_policy.md を新規作成して調査切替条件 調査順序 情報採用条件 完了条件を分離定義した
verification:
・codex_rollback_bridge でデバッグビルド成功を確認した

日時: 2026-02-23 02:56:27 JST
対象: codex_rollback_bridge
summary: 軽量再開モードの判定キーをプロジェクト宣言ファイル指定へ変更した
code_changes:
・AGENTS.md の再開判定文を再開文言基準から プロジェクト宣言_*.md 指定基準へ置換した
verification:
・codex_rollback_bridge でデバッグビルド成功を確認した

