use crate::command_template;
use crate::config;
use crate::git;
use crate::models::{AppStatus, RepoCandidate, RepoSnapshot, ScanScope, Settings};
use crate::monitor::{MonitorCommand, MonitorController, MonitorEvent};
use eframe::egui::{self, Button, Color32, Grid, RichText, ScrollArea};
use std::path::PathBuf;
use std::time::Duration;

struct CopyFeedback {
    message: String,
    is_error: bool,
}

pub struct CodexRollbackBridgeApp {
    project_root: PathBuf,
    settings: Settings,
    app_status: AppStatus,
    git_available: bool,
    root_folder_input: String,
    scan_scope: ScanScope,
    scan_results: Vec<RepoCandidate>,
    selected_scan_index: Option<usize>,
    selected_repo_path: Option<PathBuf>,
    snapshot: Option<RepoSnapshot>,
    selected_target_commit: Option<String>,
    last_error: Option<String>,
    logs: Vec<String>,
    monitor: MonitorController,
    config_contract: String,
    consecutive_failures: u32,
    show_project_change_dialog: bool,
    copy_feedback: Option<CopyFeedback>,
}

impl CodexRollbackBridgeApp {
    pub fn new(project_root: PathBuf, _font_validation: Result<PathBuf, String>) -> Self {
        let load_result = config::load_settings(&project_root);
        let mut settings = load_result.settings;
        settings.normalize();

        let default_project_root_text = project_root.to_string_lossy().to_string();
        let root_folder_input = settings
            .root_folder_path
            .clone()
            .unwrap_or_else(|| default_project_root_text.clone());

        let selected_repo_path = settings
            .selected_repo_path
            .as_ref()
            .map(PathBuf::from)
            .filter(|path| path.exists());

        let mut app = Self {
            project_root,
            settings: settings.clone(),
            app_status: AppStatus::ProjectUnselected,
            git_available: git::git_exists(),
            root_folder_input,
            scan_scope: settings.scan_scope,
            scan_results: Vec::new(),
            selected_scan_index: None,
            selected_repo_path: selected_repo_path.clone(),
            snapshot: None,
            selected_target_commit: None,
            last_error: None,
            logs: load_result.logs,
            monitor: MonitorController::new(settings.update_interval_sec),
            config_contract: config::config_contract_line(),
            consecutive_failures: 0,
            show_project_change_dialog: selected_repo_path.is_none(),
            copy_feedback: None,
        };

        if !app.git_available {
            app.app_status = AppStatus::Error;
            app.last_error = Some("git が見つかりません".to_string());
            app.log("git が見つからないため監視更新を停止しています");
        }

        if let Some(repo_path) = app.selected_repo_path.clone()
            && app.git_available
        {
            app.app_status = AppStatus::Updating;
            app.monitor.send(MonitorCommand::SetRepo(repo_path));
        }

        app.log(app.config_contract.clone());
        app
    }

    fn poll_monitor_events(&mut self) {
        while let Some(event) = self.monitor.try_recv() {
            match event {
                MonitorEvent::Snapshot(snapshot) => {
                    self.app_status = AppStatus::Selected;
                    self.last_error = None;
                    self.consecutive_failures = 0;
                    if let Some(target_full_id) = self.selected_target_commit.as_ref() {
                        let still_exists = snapshot
                            .recent_commits
                            .iter()
                            .any(|commit| &commit.full_id == target_full_id);
                        if !still_exists {
                            self.selected_target_commit = None;
                        }
                    }
                    self.snapshot = Some(snapshot);
                }
                MonitorEvent::Error {
                    message,
                    consecutive_failures,
                } => {
                    self.app_status = AppStatus::Error;
                    self.consecutive_failures = consecutive_failures;
                    let decorated_message =
                        format!("更新失敗({consecutive_failures}回連続): {message}");
                    self.last_error = Some(decorated_message.clone());
                    self.log(decorated_message);
                }
            }
        }
    }

    fn render_top_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("プロジェクト変更").clicked() {
                self.show_project_change_dialog = true;
            }
            let current_project = self
                .selected_repo_path
                .as_ref()
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_else(|| "(未選択)".to_string());
            ui.separator();
            ui.label(format!("現在プロジェクト: {current_project}"));
        });

        ui.horizontal_wrapped(|ui| {
            ui.label("更新間隔(秒):");
            let mut interval = self.settings.update_interval_sec;
            let response = ui.add(egui::DragValue::new(&mut interval).range(1..=3600));
            if response.changed() {
                self.settings.update_interval_sec = interval.clamp(1, 3600);
                self.monitor.send(MonitorCommand::SetIntervalSeconds(
                    self.settings.update_interval_sec,
                ));
                self.save_settings_with_log();
            }

            let can_manual_refresh = self.git_available && self.selected_repo_path.is_some();
            if ui
                .add_enabled(can_manual_refresh, Button::new("手動更新"))
                .clicked()
            {
                self.app_status = AppStatus::Updating;
                self.monitor.send(MonitorCommand::ManualRefresh);
            }

            ui.separator();
            ui.colored_label(
                self.status_color(),
                RichText::new(format!("状態: {}", self.app_status.label())).strong(),
            );
            ui.separator();

            let (dirty_label, dirty_color) = self.dirty_indicator();
            ui.colored_label(dirty_color, RichText::new(dirty_label).strong());
        });

        if self.consecutive_failures >= 3 {
            ui.colored_label(
                Color32::from_rgb(160, 0, 0),
                RichText::new("更新失敗が3回以上連続しています").strong(),
            );
        }

        if let Some(error) = &self.last_error {
            ui.colored_label(Color32::RED, error);
        }

        if let Some(feedback) = &self.copy_feedback {
            let color = if feedback.is_error {
                Color32::from_rgb(160, 0, 0)
            } else {
                Color32::from_rgb(0, 96, 0)
            };
            ui.colored_label(color, &feedback.message);
        }
    }

    fn render_main_content(&mut self, ui: &mut egui::Ui) {
        if self.selected_repo_path.is_none() {
            ui.label("プロジェクト未選択です。上部の「プロジェクト変更」から選択してください。");
            return;
        }

        if let Some(snapshot) = self.snapshot.clone() {
            self.render_commit_table(ui, &snapshot);
        } else {
            ui.label("監視データ未取得（更新待ち）");
        }

        ui.separator();
        self.render_commit_instruction_section(ui);
    }

    fn render_commit_table(&mut self, ui: &mut egui::Ui, snapshot: &RepoSnapshot) {
        ui.label("コミット一覧（最大50）");

        let mut clicked_target: Option<String> = None;
        ScrollArea::vertical()
            .id_salt("commit_table_scroll")
            .max_height(280.0)
            .show(ui, |ui| {
                Grid::new("commit_table_grid")
                    .num_columns(4)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("日時");
                        ui.strong("短縮ID");
                        ui.strong("メッセージ");
                        ui.strong("状態");
                        ui.end_row();

                        for commit in snapshot.recent_commits.iter().take(50) {
                            let is_target = self
                                .selected_target_commit
                                .as_deref()
                                .map(|id| id == commit.full_id)
                                .unwrap_or(false);

                            let datetime_text = if is_target {
                                RichText::new(&commit.datetime).strong()
                            } else {
                                RichText::new(&commit.datetime)
                            };

                            if ui.selectable_label(is_target, datetime_text).clicked() {
                                clicked_target = Some(commit.full_id.clone());
                            }
                            ui.label(&commit.short_id);
                            ui.label(&commit.message);
                            ui.label(self.commit_state_label(snapshot, &commit.full_id));
                            ui.end_row();
                        }
                    });
            });

        if let Some(target) = clicked_target {
            self.selected_target_commit = Some(target);
            self.copy_feedback = None;
        }
    }

    fn render_commit_instruction_section(&mut self, ui: &mut egui::Ui) {
        let can_generate = self.snapshot.is_some() && self.selected_target_commit.is_some();

        ui.horizontal(|ui| {
            if ui
                .add_enabled(can_generate, Button::new("コミット命令"))
                .clicked()
            {
                self.copy_commit_instruction();
            }

            if let Some(target) = &self.selected_target_commit {
                ui.label(format!("選択ターゲット: {target}"));
            } else {
                ui.colored_label(Color32::from_rgb(128, 96, 0), "ターゲットコミット未選択");
            }
        });
    }

    fn render_project_change_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.show_project_change_dialog;
        let mut close_requested = false;

        egui::Window::new("プロジェクト変更")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(900.0)
            .default_height(540.0)
            .show(ctx, |ui| {
                ui.label("ルートフォルダ配下をスキャンして Git リポジトリ候補を選択します。");

                ui.horizontal(|ui| {
                    ui.label("ルート:");
                    ui.text_edit_singleline(&mut self.root_folder_input);
                    if ui.button("フォルダ選択").clicked() {
                        self.pick_root_folder();
                    }
                    if ui.button("ワークスペース").clicked() {
                        self.root_folder_input = self.project_root.to_string_lossy().to_string();
                        self.save_settings_with_log();
                    }
                });

                let old_scope = self.scan_scope;
                ui.horizontal(|ui| {
                    ui.label("スキャン範囲:");
                    ui.radio_value(&mut self.scan_scope, ScanScope::Direct, "直下のみ");
                    ui.radio_value(&mut self.scan_scope, ScanScope::Depth2, "深さ2まで");
                });
                if old_scope != self.scan_scope {
                    self.save_settings_with_log();
                }

                ui.horizontal(|ui| {
                    if ui.button("スキャン実行").clicked() {
                        self.scan_repositories();
                    }
                    if ui.button("設定保存").clicked() {
                        self.save_settings_with_log();
                    }
                });

                ui.separator();
                ui.label("候補プロジェクト一覧");
                if self.scan_results.is_empty() {
                    ui.label("候補なし（スキャン実行を押してください）");
                } else {
                    ScrollArea::vertical()
                        .id_salt("project_candidate_scroll")
                        .max_height(320.0)
                        .show(ui, |ui| {
                            let mut clicked_index = None;
                            for (index, candidate) in self.scan_results.iter().enumerate() {
                                let selected = self.selected_scan_index == Some(index);
                                let label = format!(
                                    "{} | {} | {} | {}",
                                    candidate.folder_name,
                                    candidate.path.display(),
                                    candidate.current_branch,
                                    candidate.last_commit_datetime
                                );
                                if ui.selectable_label(selected, label).clicked() {
                                    clicked_index = Some(index);
                                }
                            }
                            if let Some(index) = clicked_index {
                                self.selected_scan_index = Some(index);
                            }
                        });
                }

                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(self.selected_scan_index.is_some(), Button::new("選択確定"))
                        .clicked()
                        && self.confirm_selected_repo()
                    {
                        close_requested = true;
                    }
                    if ui.button("閉じる").clicked() {
                        close_requested = true;
                    }
                });
            });

        if close_requested {
            open = false;
        }
        self.show_project_change_dialog = open;
    }

    fn render_logs(&self, ui: &mut egui::Ui) {
        ui.separator();
        ui.label("ログ");
        ScrollArea::vertical()
            .id_salt("app_log_scroll")
            .max_height(160.0)
            .show(ui, |ui| {
                for line in self.logs.iter().rev().take(80) {
                    ui.label(line);
                }
            });
    }

    fn status_color(&self) -> Color32 {
        match self.app_status {
            AppStatus::ProjectUnselected => Color32::from_rgb(0, 0, 0),
            AppStatus::Selected => Color32::from_rgb(0, 96, 0),
            AppStatus::Updating => Color32::from_rgb(128, 96, 0),
            AppStatus::Error => Color32::from_rgb(160, 0, 0),
        }
    }

    fn dirty_indicator(&self) -> (String, Color32) {
        match &self.snapshot {
            Some(snapshot) if snapshot.dirty => (
                format!(
                    "作業ツリー変更: {}",
                    Self::dirty_status_text(snapshot.dirty)
                ),
                Color32::from_rgb(160, 0, 0),
            ),
            Some(snapshot) => (
                format!(
                    "作業ツリー変更: {}",
                    Self::dirty_status_text(snapshot.dirty)
                ),
                Color32::from_rgb(0, 96, 0),
            ),
            None => (
                "作業ツリー変更: -".to_string(),
                Color32::from_rgb(64, 64, 64),
            ),
        }
    }

    fn dirty_status_text(dirty: bool) -> &'static str {
        if dirty { "変更中" } else { "変更なし" }
    }

    fn commit_state_label(&self, snapshot: &RepoSnapshot, full_id: &str) -> &'static str {
        let is_head = snapshot.head_full_id == full_id;
        let is_target = self
            .selected_target_commit
            .as_deref()
            .map(|target| target == full_id)
            .unwrap_or(false);

        match (is_head, is_target) {
            (true, true) => "HEAD/TARGET",
            (true, false) => "HEAD",
            (false, true) => "TARGET",
            (false, false) => "",
        }
    }

    fn pick_root_folder(&mut self) {
        let mut dialog = rfd::FileDialog::new();
        let current = PathBuf::from(self.root_folder_input.trim());
        if current.is_dir() {
            dialog = dialog.set_directory(current);
        }

        if let Some(folder) = dialog.pick_folder() {
            self.root_folder_input = folder.to_string_lossy().to_string();
            self.scan_results.clear();
            self.selected_scan_index = None;
            self.save_settings_with_log();
        }
    }

    fn scan_repositories(&mut self) {
        if !self.git_available {
            self.last_error = Some("git が見つかりません".to_string());
            return;
        }

        let root_path = PathBuf::from(self.root_folder_input.trim());
        if !root_path.is_dir() {
            self.last_error = Some(format!(
                "ルートフォルダが不正です: {}",
                root_path.to_string_lossy()
            ));
            return;
        }

        match git::scan_repositories(&root_path, self.scan_scope) {
            Ok(results) => {
                self.scan_results = results;
                self.selected_scan_index = None;
                self.last_error = None;
                self.log(format!("スキャン完了: {} 件", self.scan_results.len()));
                self.save_settings_with_log();
            }
            Err(err) => {
                self.last_error = Some(err.clone());
                self.log(format!("スキャン失敗: {err}"));
            }
        }
    }

    fn confirm_selected_repo(&mut self) -> bool {
        let Some(index) = self.selected_scan_index else {
            self.last_error = Some("候補プロジェクト未選択".to_string());
            return false;
        };
        let Some(candidate) = self.scan_results.get(index) else {
            self.last_error = Some("候補プロジェクトの参照に失敗しました".to_string());
            return false;
        };
        self.set_selected_repo(candidate.path.clone());
        true
    }

    fn set_selected_repo(&mut self, repo_path: PathBuf) {
        self.selected_repo_path = Some(repo_path.clone());
        self.snapshot = None;
        self.selected_target_commit = None;
        self.app_status = AppStatus::Updating;
        self.last_error = None;
        self.copy_feedback = None;

        self.monitor.send(MonitorCommand::SetRepo(repo_path));
        self.save_settings_with_log();
    }

    fn copy_commit_instruction(&mut self) {
        let Some(snapshot) = self.snapshot.clone() else {
            self.set_copy_feedback("監視データ未取得のため生成できません", true);
            self.log("コミット命令生成失敗: 監視データ未取得");
            return;
        };

        let Some(target_full_id) = self.selected_target_commit.clone() else {
            self.set_copy_feedback("ターゲットコミット未選択", true);
            self.log("コミット命令生成失敗: ターゲットコミット未選択");
            return;
        };

        let Some(target_commit) = snapshot
            .recent_commits
            .iter()
            .find(|commit| commit.full_id == target_full_id)
        else {
            self.set_copy_feedback(
                "選択中ターゲットが最新一覧に存在しません。再選択してください",
                true,
            );
            self.log("コミット命令生成失敗: ターゲットがコミット一覧に存在しない");
            return;
        };

        let instruction = command_template::build_codex_instruction(
            &snapshot,
            &target_full_id,
            &target_commit.message,
        );

        match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(instruction)) {
            Ok(()) => {
                self.last_error = None;
                self.set_copy_feedback("コミット命令をクリップボードにコピーしました", false);
                self.log("コミット命令コピー成功");
            }
            Err(err) => {
                let message = format!("クリップボードコピー失敗: {err}");
                self.last_error = Some(message.clone());
                self.set_copy_feedback(message.clone(), true);
                self.log(format!("コミット命令コピー失敗: {err}"));
            }
        }
    }

    fn set_copy_feedback(&mut self, message: impl Into<String>, is_error: bool) {
        self.copy_feedback = Some(CopyFeedback {
            message: message.into(),
            is_error,
        });
    }

    fn persist_settings(&mut self) -> Result<PathBuf, String> {
        self.settings.root_folder_path = if self.root_folder_input.trim().is_empty() {
            None
        } else {
            Some(self.root_folder_input.trim().to_string())
        };
        self.settings.scan_scope = self.scan_scope;
        self.settings.selected_repo_path = self
            .selected_repo_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string());
        self.settings.normalize();

        config::save_override(&self.settings)
    }

    fn save_settings_with_log(&mut self) {
        match self.persist_settings() {
            Ok(path) => {
                self.log(format!("設定保存: {}", path.display()));
            }
            Err(err) => {
                self.log(format!("設定保存失敗: {err}"));
            }
        }
    }

    fn log(&mut self, message: impl Into<String>) {
        self.logs.push(message.into());
        if self.logs.len() > 400 {
            let remove_count = self.logs.len() - 400;
            self.logs.drain(0..remove_count);
        }
    }
}

impl eframe::App for CodexRollbackBridgeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_monitor_events();

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            self.render_top_panel(ui);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_main_content(ui);
            self.render_logs(ui);
        });

        if self.show_project_change_dialog {
            self.render_project_change_dialog(ctx);
        }

        ctx.request_repaint_after(Duration::from_millis(200));
    }
}
