use crate::command_template;
use crate::config;
use crate::git;
use crate::models::{AppStatus, RepoCandidate, RepoSnapshot, ScanScope, Settings};
use crate::monitor::{MonitorCommand, MonitorController, MonitorEvent};
use eframe::egui::{self, Button, Color32, Frame, Grid, RichText, ScrollArea, Sense, Stroke};
use std::path::PathBuf;
use std::time::Duration;

struct CopyFeedback {
    message: String,
    is_error: bool,
}

pub struct CodexRollbackBridgeApp {
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
    project_change_dialog_was_open: bool,
    copy_feedback: Option<CopyFeedback>,
}

impl CodexRollbackBridgeApp {
    pub fn new(project_root: PathBuf, _font_validation: Result<PathBuf, String>) -> Self {
        let load_result = config::load_settings(&project_root);
        let mut settings = load_result.settings;
        settings.normalize();
        settings.scan_scope = ScanScope::Direct;

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
            settings: settings.clone(),
            app_status: AppStatus::ProjectUnselected,
            git_available: git::git_exists(),
            root_folder_input,
            scan_scope: ScanScope::Direct,
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
            project_change_dialog_was_open: false,
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
        ui.add_space(2.0);

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

        ui.add_space(4.0);

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

        ui.add_space(2.0);
        ui.separator();
        ui.add_space(0.0);
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
        self.render_rollback_section(ui);
    }

    fn render_commit_table(&mut self, ui: &mut egui::Ui, snapshot: &RepoSnapshot) {
        const COMMIT_TABLE_SLOTS: usize = 10;
        let available_width = ui.available_width();
        let table_width = if available_width >= 640.0 {
            available_width.min(940.0)
        } else {
            available_width
        };
        let side_margin = ((available_width - table_width) * 0.5).max(0.0);

        let datetime_width = 160.0;
        let short_id_width = 96.0;
        let message_width = (table_width - datetime_width - short_id_width).max(240.0);

        ui.horizontal(|ui| {
            ui.add_space(side_margin);
            ui.vertical(|ui| {
                ui.set_width(table_width);
                ui.label("コミット一覧（10枠固定）");

                let mut clicked_target: Option<String> = None;
                Grid::new("commit_table_grid")
                    .num_columns(3)
                    .spacing(egui::vec2(0.0, 0.0))
                    .show(ui, |ui| {
                        self.render_table_header_cell(ui, "日時", datetime_width);
                        self.render_table_header_cell(ui, "短縮ID", short_id_width);
                        self.render_table_header_cell(ui, "メッセージ", message_width);
                        ui.end_row();

                        for row_index in 0..COMMIT_TABLE_SLOTS {
                            if let Some(commit) = snapshot.recent_commits.get(row_index) {
                                let is_selected = self
                                    .selected_target_commit
                                    .as_deref()
                                    .map(|id| id == commit.full_id)
                                    .unwrap_or(false);
                                let is_head = snapshot.head_full_id == commit.full_id;

                                let mut row_clicked = false;
                                row_clicked |= self.render_table_selectable_cell(
                                    ui,
                                    &commit.datetime,
                                    is_selected,
                                    is_head,
                                    datetime_width,
                                );
                                row_clicked |= self.render_table_selectable_cell(
                                    ui,
                                    &commit.short_id,
                                    is_selected,
                                    is_head,
                                    short_id_width,
                                );
                                row_clicked |= self.render_table_selectable_cell(
                                    ui,
                                    &commit.message,
                                    is_selected,
                                    is_head,
                                    message_width,
                                );

                                if row_clicked {
                                    clicked_target = Some(commit.full_id.clone());
                                }
                            } else {
                                self.render_table_empty_cell(ui, datetime_width);
                                self.render_table_empty_cell(ui, short_id_width);
                                self.render_table_empty_cell(ui, message_width);
                            }
                            ui.end_row();
                        }
                    });

                if let Some(target) = clicked_target {
                    self.selected_target_commit = Some(target);
                    self.copy_feedback = None;
                }
            });
            ui.add_space(side_margin);
        });
    }

    fn render_table_header_cell(&self, ui: &mut egui::Ui, text: &str, width: f32) {
        Frame::NONE
            .fill(Color32::from_rgb(236, 236, 236))
            .stroke(Stroke::new(1.0, Color32::from_rgb(120, 120, 120)))
            .show(ui, |ui| {
                ui.add_sized(
                    [width, 26.0],
                    egui::Label::new(RichText::new(text).strong()).truncate(),
                );
            });
    }

    fn render_table_selectable_cell(
        &self,
        ui: &mut egui::Ui,
        text: &str,
        selected: bool,
        is_head: bool,
        width: f32,
    ) -> bool {
        let fill_color = if selected {
            Color32::from_rgb(236, 236, 236)
        } else if is_head {
            Color32::from_rgb(228, 245, 228)
        } else {
            Color32::from_rgb(252, 252, 252)
        };
        Frame::NONE
            .fill(fill_color)
            .stroke(Stroke::new(1.0, Color32::from_rgb(144, 144, 144)))
            .show(ui, |ui| {
                let response = ui.add_sized(
                    [width, 24.0],
                    egui::Label::new(RichText::new(text))
                        .sense(Sense::click())
                        .truncate(),
                );
                response.clicked()
            })
            .inner
    }

    fn render_table_empty_cell(&self, ui: &mut egui::Ui, width: f32) {
        Frame::NONE
            .fill(Color32::from_rgb(252, 252, 252))
            .stroke(Stroke::new(1.0, Color32::from_rgb(144, 144, 144)))
            .show(ui, |ui| {
                ui.add_sized([width, 24.0], egui::Label::new(""));
            });
    }

    fn render_rollback_section(&mut self, ui: &mut egui::Ui) {
        let has_snapshot = self.snapshot.is_some();
        let dirty = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.dirty)
            .unwrap_or(false);

        ui.horizontal(|ui| {
            if ui
                .add_enabled(has_snapshot && dirty, Button::new("編集ロールバック"))
                .clicked()
            {
                self.copy_worktree_rollback_command();
            }

            if ui
                .add_enabled(has_snapshot && !dirty, Button::new("1コミット戻す"))
                .clicked()
            {
                self.copy_previous_head_rollback_command();
            }
        });

        if !has_snapshot {
            ui.label("監視データ未取得のためコマンド生成できません");
            return;
        }

        if dirty {
            ui.label("作業ツリー変更あり: 先に「編集ロールバック」を実行してください");
        } else {
            ui.label("作業ツリー変更なし: 「1コミット戻す」を実行できます");
        }
    }

    fn render_project_change_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.show_project_change_dialog;
        let mut close_requested = false;
        let parent_size = ctx.content_rect().size();
        let dialog_size = egui::vec2(
            (parent_size.x * 0.9).max(640.0),
            (parent_size.y * 0.9).max(420.0),
        );

        egui::Window::new("プロジェクト変更")
            .open(&mut open)
            .collapsible(false)
            .movable(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .fixed_size(dialog_size)
            .show(ctx, |ui| {
                ui.label("ルートフォルダ配下をスキャンして Git リポジトリ候補を選択します。");

                ui.horizontal(|ui| {
                    ui.label("ルート:");
                    ui.text_edit_singleline(&mut self.root_folder_input);
                    if ui.button("フォルダ選択").clicked() {
                        self.pick_root_folder();
                    }
                });

                ui.separator();
                ui.label("候補プロジェクト一覧");
                if self.scan_results.is_empty() {
                    ui.label("候補なし");
                } else {
                    let total_width = ui.available_width().max(700.0);
                    let title_width = 240.0;
                    let datetime_width = 160.0;
                    let path_width = (total_width - title_width - datetime_width).max(280.0);
                    ScrollArea::vertical()
                        .id_salt("project_candidate_scroll")
                        .max_height(320.0)
                        .show(ui, |ui| {
                            Grid::new("project_candidate_grid")
                                .num_columns(3)
                                .spacing(egui::vec2(0.0, 0.0))
                                .show(ui, |ui| {
                                    self.render_table_header_cell(ui, "要件定義1行目", title_width);
                                    self.render_table_header_cell(ui, "パス", path_width);
                                    self.render_table_header_cell(
                                        ui,
                                        "最終コミット",
                                        datetime_width,
                                    );
                                    ui.end_row();

                                    let mut clicked_index = None;
                                    for (index, candidate) in self.scan_results.iter().enumerate() {
                                        let selected = self.selected_scan_index == Some(index);
                                        let mut row_clicked = false;
                                        row_clicked |= self.render_table_selectable_cell(
                                            ui,
                                            &candidate.requirement_headline,
                                            selected,
                                            false,
                                            title_width,
                                        );
                                        row_clicked |= self.render_table_selectable_cell(
                                            ui,
                                            &candidate.path.to_string_lossy(),
                                            selected,
                                            false,
                                            path_width,
                                        );
                                        row_clicked |= self.render_table_selectable_cell(
                                            ui,
                                            &candidate.last_commit_datetime,
                                            selected,
                                            false,
                                            datetime_width,
                                        );
                                        if row_clicked {
                                            clicked_index = Some(index);
                                        }
                                        ui.end_row();
                                    }

                                    if let Some(index) = clicked_index {
                                        self.selected_scan_index = Some(index);
                                    }
                                });
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
            self.scan_repositories();
        }
    }

    fn scan_repositories(&mut self) {
        self.scan_scope = ScanScope::Direct;
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

        match git::scan_repositories(&root_path, ScanScope::Direct) {
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

    fn copy_worktree_rollback_command(&mut self) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            self.set_copy_feedback("監視データ未取得のため生成できません", true);
            self.log("編集ロールバック生成失敗: 監視データ未取得");
            return;
        };

        let instruction = command_template::build_worktree_rollback_command(snapshot);

        match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(instruction)) {
            Ok(()) => {
                self.last_error = None;
                self.set_copy_feedback(
                    "編集ロールバックコマンドをクリップボードにコピーしました",
                    false,
                );
                self.log("編集ロールバックコマンドコピー成功");
            }
            Err(err) => {
                let message = format!("クリップボードコピー失敗: {err}");
                self.last_error = Some(message.clone());
                self.set_copy_feedback(message.clone(), true);
                self.log(format!("編集ロールバックコマンドコピー失敗: {err}"));
            }
        }
    }

    fn copy_previous_head_rollback_command(&mut self) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            self.set_copy_feedback("監視データ未取得のため生成できません", true);
            self.log("1コミット戻す生成失敗: 監視データ未取得");
            return;
        };

        if snapshot.dirty {
            self.set_copy_feedback(
                "作業ツリー変更があるため先に「編集ロールバック」を実行してください",
                true,
            );
            self.log("1コミット戻す生成失敗: 作業ツリー変更あり");
            return;
        }

        let instruction = command_template::build_previous_head_rollback_command(snapshot);

        match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(instruction)) {
            Ok(()) => {
                self.last_error = None;
                self.set_copy_feedback(
                    "1コミット戻すコマンドをクリップボードにコピーしました",
                    false,
                );
                self.log("1コミット戻すコマンドコピー成功");
            }
            Err(err) => {
                let message = format!("クリップボードコピー失敗: {err}");
                self.last_error = Some(message.clone());
                self.set_copy_feedback(message.clone(), true);
                self.log(format!("1コミット戻すコマンドコピー失敗: {err}"));
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
        let was_dialog_open = self.project_change_dialog_was_open;

        egui::TopBottomPanel::top("top_panel")
            .show_separator_line(false)
            .show(ctx, |ui| {
                self.render_top_panel(ui);
            });

        if self.show_project_change_dialog && !was_dialog_open {
            self.scan_repositories();
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_main_content(ui);
        });

        if self.show_project_change_dialog {
            self.render_project_change_dialog(ctx);
        }
        self.project_change_dialog_was_open = self.show_project_change_dialog;

        let _ = self.logs.len();
        ctx.request_repaint_after(Duration::from_millis(200));
    }
}
