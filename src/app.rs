use crate::config;
use crate::git;
use crate::models::{
    AppStatus, RepoCandidate, RepoSnapshot, ScanScope, Settings, WorkState,
};
use crate::monitor::{MonitorCommand, MonitorController, MonitorEvent};
use eframe::egui::{self, Button, Color32, Frame, Grid, RichText, ScrollArea, Sense, Stroke};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

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
    commit_scroll_offset_y: f32,
    commit_scroll_point_remainder: f32,
    commit_scroll_line_remainder: f32,
    show_project_change_dialog: bool,
    project_change_dialog_was_open: bool,
    copy_feedback: Option<CopyFeedback>,
    show_commit_confirm_dialog: bool,
    show_revert_confirm_dialog: bool,
    revert_confirm_deadline: Option<Instant>,
    show_main_head_confirm_dialog: bool,
    main_head_confirm_deadline: Option<Instant>,
    show_commit_message_dialog: bool,
    commit_message_dialog_text: Option<String>,
    next_external_repo_poll_at: Instant,
    last_external_repo_file_value: Option<Option<String>>,
    last_external_repo_poll_path: Option<PathBuf>,
    last_external_repo_modified_at: Option<Option<SystemTime>>,
    last_external_repo_read_error: Option<String>,
    external_repo_path_info: Option<config::ExternalSelectedRepoPathInfo>,
    external_repo_force_fallback: bool,
}

impl CodexRollbackBridgeApp {
    const REVERT_CONFIRM_COUNTDOWN_SECONDS: u64 = 5;
    const REVERT_CONFIRM_ZERO_DISPLAY_MILLIS: u64 = 800;
    const EXTERNAL_REPO_POLL_INTERVAL: Duration = Duration::from_secs(1);

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
            commit_scroll_offset_y: 0.0,
            commit_scroll_point_remainder: 0.0,
            commit_scroll_line_remainder: 0.0,
            show_project_change_dialog: selected_repo_path.is_none(),
            project_change_dialog_was_open: false,
            copy_feedback: None,
            show_commit_confirm_dialog: false,
            show_revert_confirm_dialog: false,
            revert_confirm_deadline: None,
            show_main_head_confirm_dialog: false,
            main_head_confirm_deadline: None,
            show_commit_message_dialog: false,
            commit_message_dialog_text: None,
            next_external_repo_poll_at: Instant::now(),
            last_external_repo_file_value: None,
            last_external_repo_poll_path: None,
            last_external_repo_modified_at: None,
            last_external_repo_read_error: None,
            external_repo_path_info: None,
            external_repo_force_fallback: false,
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
        app.poll_external_selected_repo_path(true);
        if app.selected_repo_path.is_some() {
            app.save_external_selected_repo_with_log(true);
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

    fn poll_external_selected_repo_path(&mut self, force: bool) {
        let now = Instant::now();
        if !force && now < self.next_external_repo_poll_at {
            return;
        }
        self.next_external_repo_poll_at = now + Self::EXTERNAL_REPO_POLL_INTERVAL;

        let path_info = match config::resolve_external_selected_repo_path(self.external_repo_force_fallback)
        {
            Ok(path_info) => path_info,
            Err(err) => {
                let should_log = self
                    .last_external_repo_read_error
                    .as_ref()
                    .map(|previous| previous != &err)
                    .unwrap_or(true);
                if should_log {
                    self.log(format!("外部選択ファイルパス解決失敗: {err}"));
                }
                self.last_external_repo_read_error = Some(err);
                return;
            }
        };
        self.external_repo_path_info = Some(path_info.clone());

        let modified_at = match fs::metadata(&path_info.path) {
            Ok(metadata) => match metadata.modified() {
                Ok(modified_at) => Some(modified_at),
                Err(err) => {
                    let message = format!(
                        "外部選択ファイル更新時刻取得失敗 '{}': {err}",
                        path_info.path.display()
                    );
                    let should_log = self
                        .last_external_repo_read_error
                        .as_ref()
                        .map(|previous| previous != &message)
                        .unwrap_or(true);
                    if should_log {
                        self.log(message.clone());
                    }
                    self.last_external_repo_read_error = Some(message);
                    return;
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => {
                let message =
                    format!("外部選択ファイル状態取得失敗 '{}': {err}", path_info.path.display());
                let should_log = self
                    .last_external_repo_read_error
                    .as_ref()
                    .map(|previous| previous != &message)
                    .unwrap_or(true);
                if should_log {
                    self.log(message.clone());
                }
                self.last_external_repo_read_error = Some(message);
                return;
            }
        };

        let path_changed = self
            .last_external_repo_poll_path
            .as_ref()
            .map(|previous| previous != &path_info.path)
            .unwrap_or(true);
        let modified_changed = self
            .last_external_repo_modified_at
            .as_ref()
            .map(|previous| previous != &modified_at)
            .unwrap_or(true);

        if !force && !path_changed && !modified_changed {
            return;
        }

        self.last_external_repo_poll_path = Some(path_info.path.clone());
        self.last_external_repo_modified_at = Some(modified_at);

        let external_value = match config::load_external_selected_repo_path_from_file(&path_info.path)
        {
            Ok(value) => {
                self.last_external_repo_read_error = None;
                value
            }
            Err(err) => {
                let should_log = self
                    .last_external_repo_read_error
                    .as_ref()
                    .map(|previous| previous != &err)
                    .unwrap_or(true);
                if should_log {
                    self.log(format!("外部選択ファイル読込失敗: {err}"));
                }
                self.last_external_repo_read_error = Some(err);
                return;
            }
        };

        let changed = self
            .last_external_repo_file_value
            .as_ref()
            .map(|previous| previous != &external_value)
            .unwrap_or(true);
        if !changed {
            return;
        }
        self.last_external_repo_file_value = Some(external_value.clone());

        let Some(raw_path) = external_value else {
            return;
        };

        let candidate = PathBuf::from(raw_path.trim());
        if !candidate.is_dir() || !git::is_git_repo(&candidate) {
            self.log(format!(
                "外部選択ファイルの値を無視しました: {}",
                candidate.display()
            ));
            return;
        }

        if self.selected_repo_path.as_ref() == Some(&candidate) {
            return;
        }

        self.log(format!(
            "外部選択ファイルの変更を反映: {}",
            candidate.display()
        ));
        self.set_selected_repo(candidate);
    }

    fn render_top_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(2.0);

        ui.horizontal_wrapped(|ui| {
            if ui.button("プロジェクト変更").clicked() {
                self.show_project_change_dialog = true;
            }
            let current_project_path = self.selected_repo_path.as_ref().cloned();
            let project_name = current_project_path
                .as_ref()
                .and_then(|path| git::read_project_name_from_declaration(path))
                .or_else(|| {
                    current_project_path
                        .as_ref()
                        .and_then(|path| path.file_name())
                        .map(|name| name.to_string_lossy().to_string())
                })
                .unwrap_or_else(|| "(未選択)".to_string());
            let folder_name = current_project_path
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "-".to_string());
            ui.separator();
            ui.label("現在のプロジェクト名:");
            ui.label(project_name);
            ui.separator();
            ui.label(format!("フォルダ: {folder_name}"));
        });
        if let Some(path_info) = &self.external_repo_path_info {
            let suffix = if path_info.using_fallback {
                " (fallback)"
            } else {
                ""
            };
            ui.label(format!(
                "外部連携ファイル: {}{}",
                path_info.path.display(),
                suffix
            ));
        }

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

            let can_commit = self.can_run_commit_action();
            let commit_text = if can_commit {
                RichText::new("コミット")
            } else {
                RichText::new("コミット").color(Color32::from_gray(140))
            };
            if ui
                .add_enabled(can_commit, Button::new(commit_text))
                .clicked()
            {
                self.show_commit_confirm_dialog = true;
            }
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

        let feedback_line_height = ui.text_style_height(&egui::TextStyle::Body);
        let (feedback_text, feedback_color) = if let Some(feedback) = &self.copy_feedback {
            let color = if feedback.is_error {
                Color32::from_rgb(160, 0, 0)
            } else {
                Color32::from_rgb(0, 96, 0)
            };
            (feedback.message.as_str(), color)
        } else {
            ("", Color32::TRANSPARENT)
        };
        ui.add_sized(
            [ui.available_width(), feedback_line_height],
            egui::Label::new(RichText::new(feedback_text).color(feedback_color)).truncate(),
        );

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
        self.render_rollback_status(ui);
    }

    fn render_commit_table(&mut self, ui: &mut egui::Ui, snapshot: &RepoSnapshot) {
        const VISIBLE_ROWS: usize = 10;
        const ROW_OUTER_HEIGHT: f32 = 26.0;
        const VIEWPORT_TRIM_PX: f32 = 2.0;
        const MAX_COMMITS: usize = 50;
        let available_width = ui.available_width();
        let table_width = if available_width >= 700.0 {
            available_width.min(990.0)
        } else {
            available_width
        };
        let side_margin = ((available_width - table_width) * 0.5).max(0.0);

        let scope_width = 52.0;
        let datetime_width = 160.0 * 2.0 / 3.0;
        let short_id_width = 96.0;
        let message_width = (table_width * 2.0 / 3.0).max(210.0);
        let can_run_history_action = self.can_run_history_action(snapshot);

        ui.horizontal(|ui| {
            ui.add_space(side_margin);
            ui.vertical(|ui| {
                ui.set_width(table_width);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label("コミット一覧（表示10行 / 最大50件）");
                        ui.label("凡例: M=main履歴 / H=HEAD（両方=MH）");
                    });
                    ui.add_space((table_width - 500.0).max(8.0));
                    self.render_rollback_buttons(ui, can_run_history_action);
                });

                ui.scope(|ui| {
                    // Keep table row height deterministic even when global interact size is larger.
                    ui.style_mut().spacing.interact_size.y = 0.0;

                    // Account for frame stroke so 10 rows are fully visible without clipping.
                    let viewport_height =
                        (VISIBLE_ROWS as f32 * ROW_OUTER_HEIGHT - VIEWPORT_TRIM_PX).max(0.0);
                    let display_count =
                        Self::commit_display_row_count(snapshot.recent_commits.len(), VISIBLE_ROWS);
                    let commits: Vec<_> =
                        snapshot.recent_commits.iter().take(MAX_COMMITS).collect();
                    let mut clicked_target: Option<String> = None;
                    let mut clicked_message: Option<(String, String)> = None;

                    Grid::new("commit_table_header_grid")
                        .num_columns(4)
                        .spacing(egui::vec2(0.0, 0.0))
                        .show(ui, |ui| {
                            self.render_table_header_cell(ui, "区分", scope_width);
                            self.render_table_header_cell(ui, "日時", datetime_width);
                            self.render_table_header_cell(ui, "短縮ID", short_id_width);
                            self.render_table_header_cell(ui, "メッセージ", message_width);
                            ui.end_row();
                        });

                    ui.allocate_ui_with_layout(
                        egui::vec2(table_width, viewport_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            let scroll_rect = egui::Rect::from_min_size(
                                ui.cursor().min,
                                egui::vec2(table_width, viewport_height),
                            );
                            let pointer_on_table = ui.rect_contains_pointer(scroll_rect);
                            let wheel_rows = if pointer_on_table {
                                self.collect_commit_wheel_rows(ui, ROW_OUTER_HEIGHT, VISIBLE_ROWS)
                            } else {
                                0
                            };
                            if pointer_on_table && wheel_rows != 0 {
                                self.commit_scroll_offset_y = (self.commit_scroll_offset_y
                                    + wheel_rows as f32 * ROW_OUTER_HEIGHT)
                                    .max(0.0);
                                ui.ctx().input_mut(|input| {
                                    input.smooth_scroll_delta = egui::Vec2::ZERO;
                                    input.raw_scroll_delta = egui::Vec2::ZERO;
                                });
                            }

                            let output = ScrollArea::vertical()
                                .id_salt("commit_table_scroll")
                                .max_height(viewport_height)
                                .vertical_scroll_offset(self.commit_scroll_offset_y)
                                .show(ui, |ui| {
                                    Grid::new("commit_table_rows_grid")
                                        .num_columns(4)
                                        .spacing(egui::vec2(0.0, 0.0))
                                        .show(ui, |ui| {
                                            for row_index in 0..display_count {
                                                if let Some(commit) = commits.get(row_index) {
                                                    let is_selected = self
                                                        .selected_target_commit
                                                        .as_deref()
                                                        .map(|id| id == commit.full_id)
                                                        .unwrap_or(false);
                                                    let is_head =
                                                        snapshot.head_full_id == commit.full_id;
                                                    let is_master_scope = commit.in_main_history;

                                                    let mut row_clicked = false;
                                                    row_clicked |= self
                                                        .render_table_selectable_cell(
                                                            ui,
                                                            &commit.scope_mark,
                                                            is_selected,
                                                            is_master_scope,
                                                            is_head,
                                                            scope_width,
                                                            false,
                                                        );
                                                    row_clicked |= self
                                                        .render_table_selectable_cell(
                                                            ui,
                                                            &commit.datetime,
                                                            is_selected,
                                                            is_master_scope,
                                                            is_head,
                                                            datetime_width,
                                                            true,
                                                        );
                                                    row_clicked |= self
                                                        .render_table_selectable_cell(
                                                            ui,
                                                            &commit.short_id,
                                                            is_selected,
                                                            is_master_scope,
                                                            is_head,
                                                            short_id_width,
                                                            false,
                                                        );
                                                    let message_clicked = self
                                                        .render_table_selectable_cell(
                                                            ui,
                                                            &commit.subject,
                                                            is_selected,
                                                            is_master_scope,
                                                            is_head,
                                                            message_width,
                                                            false,
                                                        );
                                                    row_clicked |= message_clicked;

                                                    if row_clicked {
                                                        clicked_target =
                                                            Some(commit.full_id.clone());
                                                        clicked_message = Some((
                                                            commit.full_id.clone(),
                                                            commit.body_full.clone(),
                                                        ));
                                                    }
                                                } else {
                                                    self.render_table_empty_cell(ui, scope_width);
                                                    self.render_table_empty_cell(
                                                        ui,
                                                        datetime_width,
                                                    );
                                                    self.render_table_empty_cell(
                                                        ui,
                                                        short_id_width,
                                                    );
                                                    self.render_table_empty_cell(ui, message_width);
                                                }
                                                ui.end_row();
                                            }
                                        });
                                });

                            let max_offset = (display_count as f32 * ROW_OUTER_HEIGHT
                                - viewport_height)
                                .max(0.0);
                            self.commit_scroll_offset_y =
                                output.state.offset.y.clamp(0.0, max_offset);
                        },
                    );

                    if let Some(target) = clicked_target {
                        self.selected_target_commit = Some(target);
                        self.copy_feedback = None;
                    }
                    if let Some((commit_id, fallback_message)) = clicked_message {
                        self.open_commit_message_dialog_for_commit(
                            &snapshot.repo_path,
                            &commit_id,
                            fallback_message,
                        );
                    }
                });
            });
            ui.add_space(side_margin);
        });
    }

    fn commit_display_row_count(commit_count: usize, visible_rows: usize) -> usize {
        let bounded = commit_count.min(50);
        bounded.max(visible_rows)
    }

    fn collect_commit_wheel_rows(
        &mut self,
        ui: &egui::Ui,
        row_outer_height: f32,
        visible_rows: usize,
    ) -> i32 {
        let mut rows = 0i32;
        let events = ui.ctx().input(|input| input.events.clone());
        for event in events {
            if let egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
            } = event
            {
                if modifiers.ctrl || modifiers.command {
                    continue;
                }
                match unit {
                    egui::MouseWheelUnit::Line => {
                        self.commit_scroll_line_remainder += -delta.y;
                        while self.commit_scroll_line_remainder >= 1.0 {
                            rows += 1;
                            self.commit_scroll_line_remainder -= 1.0;
                        }
                        while self.commit_scroll_line_remainder <= -1.0 {
                            rows -= 1;
                            self.commit_scroll_line_remainder += 1.0;
                        }
                    }
                    egui::MouseWheelUnit::Point => {
                        self.commit_scroll_point_remainder += -delta.y;
                        while self.commit_scroll_point_remainder >= row_outer_height {
                            rows += 1;
                            self.commit_scroll_point_remainder -= row_outer_height;
                        }
                        while self.commit_scroll_point_remainder <= -row_outer_height {
                            rows -= 1;
                            self.commit_scroll_point_remainder += row_outer_height;
                        }
                    }
                    egui::MouseWheelUnit::Page => {
                        rows += (-delta.y).round() as i32 * visible_rows as i32;
                    }
                }
            }
        }
        rows
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
        is_master_scope: bool,
        is_head: bool,
        width: f32,
        center: bool,
    ) -> bool {
        let fill_color = Self::table_row_fill_color(selected, is_master_scope, is_head);
        Frame::NONE
            .fill(fill_color)
            .stroke(Stroke::new(1.0, Color32::from_rgb(144, 144, 144)))
            .show(ui, |ui| {
                let response = if center {
                    ui.with_layout(
                        egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                        |ui| {
                            ui.add_sized(
                                [width, 24.0],
                                egui::Label::new(RichText::new(text))
                                    .sense(Sense::click())
                                    .truncate(),
                            )
                        },
                    )
                    .inner
                } else {
                    ui.add_sized(
                        [width, 24.0],
                        egui::Label::new(RichText::new(text))
                            .sense(Sense::click())
                            .truncate(),
                    )
                };
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

    fn table_row_fill_color(selected: bool, is_master_scope: bool, is_head: bool) -> Color32 {
        if selected {
            Color32::from_rgb(236, 236, 236)
        } else if is_head {
            Color32::from_rgb(255, 224, 224)
        } else if is_master_scope {
            Color32::from_rgb(228, 245, 228)
        } else {
            Color32::from_rgb(252, 252, 252)
        }
    }

    fn render_rollback_buttons(&mut self, ui: &mut egui::Ui, can_run_history_action: bool) {
        ui.horizontal(|ui| {
            let previous_head_text = if can_run_history_action {
                RichText::new("ロルバ")
            } else {
                RichText::new("ロルバ").color(Color32::from_gray(140))
            };
            if ui
                .add_enabled(can_run_history_action, Button::new(previous_head_text))
                .clicked()
            {
                self.open_revert_confirm_dialog();
            }

            let main_to_head_text = if can_run_history_action {
                RichText::new("mainをHEADにする")
            } else {
                RichText::new("mainをHEADにする").color(Color32::from_gray(140))
            };
            if ui
                .add_enabled(can_run_history_action, Button::new(main_to_head_text))
                .clicked()
            {
                self.open_main_head_confirm_dialog();
            }
        });
    }

    fn render_rollback_status(&mut self, ui: &mut egui::Ui) {
        let _ = ui;
    }

    fn open_commit_message_dialog(&mut self, message: String) {
        self.commit_message_dialog_text = Some(message);
        self.show_commit_message_dialog = true;
    }

    fn open_commit_message_dialog_for_commit(
        &mut self,
        repo_path: &Path,
        commit_id: &str,
        fallback_message: String,
    ) {
        let message = match git::get_commit_message_full(repo_path, commit_id) {
            Ok(full_message) => full_message,
            Err(err) => {
                self.log(format!(
                    "コミット全文取得失敗: id={commit_id} reason={err} (fallback使用)"
                ));
                fallback_message
            }
        };
        let normalized_message = Self::normalize_message_newlines(message);
        self.log(format!(
            "commit message lines={}",
            normalized_message.matches('\n').count() + 1
        ));
        self.open_commit_message_dialog(normalized_message);
    }

    fn normalize_message_newlines(message: String) -> String {
        if !message.contains('\r') {
            return message;
        }
        let normalized = message.replace("\r\n", "\n");
        normalized.replace('\r', "\n")
    }

    fn render_commit_message_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_commit_message_dialog {
            return;
        }

        let mut open = self.show_commit_message_dialog;
        let mut close_requested = false;
        let parent_size = ctx.content_rect().size();
        let dialog_size = egui::vec2(
            (parent_size.x * 0.78).clamp(560.0, 960.0),
            (parent_size.y * 0.42).clamp(220.0, 360.0),
        );
        let message = self
            .commit_message_dialog_text
            .clone()
            .unwrap_or_else(|| "(メッセージなし)".to_string());

        egui::Window::new("コミットメッセージ全文")
            .open(&mut open)
            .collapsible(false)
            .movable(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 12.0))
            .fixed_size(dialog_size)
            .show(ctx, |ui| {
                ui.label("選択したコミットのメッセージ全文");
                ui.separator();

                ScrollArea::vertical()
                    .id_salt("commit_message_dialog_scroll")
                    .max_height((dialog_size.y - 92.0).max(96.0))
                    .show(ui, |ui| {
                        ui.add(egui::Label::new(message.clone()).wrap());
                    });

                ui.add_space(6.0);
                if ui.button("閉じる").clicked() {
                    close_requested = true;
                }
            });

        if close_requested {
            open = false;
        }
        if !open {
            self.commit_message_dialog_text = None;
        }
        self.show_commit_message_dialog = open;
    }

    fn render_commit_confirm_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_commit_confirm_dialog {
            return;
        }

        let mut open = self.show_commit_confirm_dialog;
        let mut close_requested = false;

        egui::Window::new("コミット確認")
            .open(&mut open)
            .collapsible(false)
            .movable(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .fixed_size(egui::vec2(420.0, 120.0))
            .show(ctx, |ui| {
                ui.label("未コミットの作業をコミットしますか。");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("はい").clicked() {
                        self.commit_selected_repo();
                        close_requested = true;
                    }
                    if ui.button("いいえ").clicked() {
                        close_requested = true;
                    }
                });
            });

        if close_requested {
            open = false;
        }
        self.show_commit_confirm_dialog = open;
    }

    fn open_revert_confirm_dialog(&mut self) {
        self.show_revert_confirm_dialog = true;
        self.revert_confirm_deadline =
            Some(Instant::now() + Duration::from_secs(Self::REVERT_CONFIRM_COUNTDOWN_SECONDS));
    }

    fn open_main_head_confirm_dialog(&mut self) {
        self.show_main_head_confirm_dialog = true;
        self.main_head_confirm_deadline =
            Some(Instant::now() + Duration::from_secs(Self::REVERT_CONFIRM_COUNTDOWN_SECONDS));
    }

    fn revert_confirm_state(&self) -> (String, bool) {
        let Some(deadline) = self.revert_confirm_deadline else {
            return ("OK".to_string(), true);
        };
        Self::revert_confirm_state_from_deadline(deadline, Instant::now())
    }

    fn main_head_confirm_state(&self) -> (String, bool) {
        let Some(deadline) = self.main_head_confirm_deadline else {
            return ("OK".to_string(), true);
        };
        Self::revert_confirm_state_from_deadline(deadline, Instant::now())
    }

    fn revert_confirm_state_from_deadline(deadline: Instant, now: Instant) -> (String, bool) {
        if now < deadline {
            let remaining = deadline.duration_since(now);
            let remaining_secs = ((remaining.as_millis() + 999) / 1000) as u64;
            return (remaining_secs.to_string(), false);
        }

        if now.duration_since(deadline)
            < Duration::from_millis(Self::REVERT_CONFIRM_ZERO_DISPLAY_MILLIS)
        {
            return ("0".to_string(), false);
        }

        ("OK".to_string(), true)
    }

    fn render_revert_confirm_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_revert_confirm_dialog {
            return;
        }

        let mut open = self.show_revert_confirm_dialog;
        let mut close_requested = false;
        let (countdown_text, can_confirm) = self.revert_confirm_state();

        egui::Window::new("ロルバ確認")
            .open(&mut open)
            .collapsible(false)
            .movable(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .fixed_size(egui::vec2(440.0, 140.0))
            .show(ctx, |ui| {
                ui.label("HEADをロルバしてよいですか。");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("カウントダウン:");
                    ui.label(RichText::new(countdown_text.as_str()).strong());
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let yes_text = if can_confirm {
                        RichText::new("はい")
                    } else {
                        RichText::new("はい").color(Color32::from_gray(140))
                    };
                    if ui.add_enabled(can_confirm, Button::new(yes_text)).clicked() {
                        self.revert_head_selected_repo();
                        close_requested = true;
                    }
                    if ui.button("いいえ").clicked() {
                        close_requested = true;
                    }
                });
            });

        if close_requested {
            open = false;
        }
        if !open {
            self.revert_confirm_deadline = None;
        }
        self.show_revert_confirm_dialog = open;
    }

    fn render_main_head_confirm_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_main_head_confirm_dialog {
            return;
        }

        let mut open = self.show_main_head_confirm_dialog;
        let mut close_requested = false;
        let (countdown_text, can_confirm) = self.main_head_confirm_state();

        egui::Window::new("main更新確認")
            .open(&mut open)
            .collapsible(false)
            .movable(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .fixed_size(egui::vec2(440.0, 140.0))
            .show(ctx, |ui| {
                ui.label("mainブランチをHEADに変更してよいですか。");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("カウントダウン:");
                    ui.label(RichText::new(countdown_text.as_str()).strong());
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let yes_text = if can_confirm {
                        RichText::new("はい")
                    } else {
                        RichText::new("はい").color(Color32::from_gray(140))
                    };
                    if ui.add_enabled(can_confirm, Button::new(yes_text)).clicked() {
                        self.set_main_branch_to_head_selected_repo();
                        close_requested = true;
                    }
                    if ui.button("いいえ").clicked() {
                        close_requested = true;
                    }
                });
            });

        if close_requested {
            open = false;
        }
        if !open {
            self.main_head_confirm_deadline = None;
        }
        self.show_main_head_confirm_dialog = open;
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
                    let datetime_width = 160.0 * 2.0 / 3.0;
                    let path_width = (total_width - title_width - datetime_width).max(280.0);
                    ScrollArea::vertical()
                        .id_salt("project_candidate_scroll")
                        .max_height(320.0)
                        .show(ui, |ui| {
                            Grid::new("project_candidate_grid")
                                .num_columns(3)
                                .spacing(egui::vec2(0.0, 0.0))
                                .show(ui, |ui| {
                                    self.render_table_header_cell(ui, "プロジェクト名", title_width);
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
                                            &candidate.project_name,
                                            selected,
                                            false,
                                            false,
                                            title_width,
                                            false,
                                        );
                                        row_clicked |= self.render_table_selectable_cell(
                                            ui,
                                            &candidate.path.to_string_lossy(),
                                            selected,
                                            false,
                                            false,
                                            path_width,
                                            false,
                                        );
                                        row_clicked |= self.render_table_selectable_cell(
                                            ui,
                                            &candidate.last_commit_datetime,
                                            selected,
                                            false,
                                            false,
                                            datetime_width,
                                            true,
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
                    let can_confirm = self.selected_scan_index.is_some();
                    let confirm_text = if can_confirm {
                        RichText::new("選択確定")
                    } else {
                        RichText::new("選択確定").color(Color32::from_gray(140))
                    };
                    if ui
                        .add_enabled(can_confirm, Button::new(confirm_text))
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

    fn displayed_work_state(&self) -> WorkState {
        if self.app_status == AppStatus::Error {
            return WorkState::Unknown;
        }
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.work_state)
            .unwrap_or(WorkState::Unknown)
    }

    fn can_run_commit_action(&self) -> bool {
        if !self.git_available || self.app_status == AppStatus::Error {
            return false;
        }
        let Some(snapshot) = self.snapshot.as_ref() else {
            return false;
        };
        if snapshot.operation.is_some() {
            return false;
        }
        self.displayed_work_state() == WorkState::Dirty
    }

    fn can_run_history_action(&self, snapshot: &RepoSnapshot) -> bool {
        self.app_status != AppStatus::Error
            && snapshot.work_state == WorkState::Clean
            && snapshot.operation.is_none()
    }

    fn history_action_block_reason(&self, snapshot: &RepoSnapshot) -> Option<&'static str> {
        if self.app_status == AppStatus::Error {
            return Some("作業状態が不明のため「ロルバ」は実行できません");
        }
        if snapshot.operation.is_some() {
            return Some("Git操作中のため「ロルバ」は実行できません");
        }
        match snapshot.work_state {
            WorkState::Clean => None,
            WorkState::Dirty => Some("未コミット変更があるため「ロルバ」は実行できません"),
            WorkState::Unknown => Some("作業状態が不明のため「ロルバ」は実行できません"),
        }
    }

    fn move_main_action_block_reason(&self, snapshot: &RepoSnapshot) -> Option<&'static str> {
        if self.app_status == AppStatus::Error {
            return Some("作業状態が不明のため「main更新」は実行できません");
        }
        if snapshot.operation.is_some() {
            return Some("Git操作中のため「main更新」は実行できません");
        }
        match snapshot.work_state {
            WorkState::Clean => None,
            WorkState::Dirty => Some("未コミット変更があるため「main更新」は実行できません"),
            WorkState::Unknown => Some("作業状態が不明のため「main更新」は実行できません"),
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
        self.show_commit_confirm_dialog = false;
        self.show_revert_confirm_dialog = false;
        self.revert_confirm_deadline = None;
        self.show_main_head_confirm_dialog = false;
        self.main_head_confirm_deadline = None;
        self.show_commit_message_dialog = false;
        self.commit_message_dialog_text = None;
        self.commit_scroll_offset_y = 0.0;
        self.commit_scroll_point_remainder = 0.0;
        self.commit_scroll_line_remainder = 0.0;
        self.app_status = AppStatus::Updating;
        self.last_error = None;
        self.copy_feedback = None;

        self.monitor.send(MonitorCommand::SetRepo(repo_path));
        self.save_settings_with_log();
        self.save_external_selected_repo_with_log(false);
    }

    fn commit_selected_repo(&mut self) {
        let Some(repo_path) = self.selected_repo_path.clone() else {
            let message = "プロジェクト未選択のためコミットできません".to_string();
            self.last_error = Some(message.clone());
            self.set_copy_feedback(message.clone(), true);
            self.log(format!("コミット失敗: {message}"));
            return;
        };

        self.app_status = AppStatus::Updating;
        match git::commit_with_runtime_message(&repo_path) {
            Ok(short_id) => {
                let message = format!("コミット完了: {short_id}");
                self.last_error = None;
                self.set_copy_feedback(message.clone(), false);
                self.log(message);
            }
            Err(err) => {
                let message = format!("コミット失敗: {err}");
                self.last_error = Some(message.clone());
                self.set_copy_feedback(message.clone(), true);
                self.log(message);
            }
        }

        self.monitor.send(MonitorCommand::ManualRefresh);
    }

    fn revert_head_selected_repo(&mut self) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            self.set_copy_feedback("監視データ未取得のためロルバできません", true);
            self.log("ロルバ失敗: 監視データ未取得");
            return;
        };

        if let Some(reason) = self.history_action_block_reason(snapshot) {
            self.set_copy_feedback(reason, true);
            self.log(format!("ロルバ失敗: {reason}"));
            return;
        }

        let Some(repo_path) = self.selected_repo_path.clone() else {
            let message = "プロジェクト未選択のためロルバできません".to_string();
            self.last_error = Some(message.clone());
            self.set_copy_feedback(message.clone(), true);
            self.log(format!("ロルバ失敗: {message}"));
            return;
        };

        self.app_status = AppStatus::Updating;
        match git::reset_head(&repo_path) {
            Ok(short_id) => {
                let message = format!("ロルバ完了: {short_id}");
                self.last_error = None;
                self.set_copy_feedback(message.clone(), false);
                self.log(message);
            }
            Err(err) => {
                let message = format!("ロルバ失敗: {err}");
                self.last_error = Some(message.clone());
                self.set_copy_feedback(message.clone(), true);
                self.log(message);
            }
        }

        self.monitor.send(MonitorCommand::ManualRefresh);
    }

    fn set_main_branch_to_head_selected_repo(&mut self) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            self.set_copy_feedback("監視データ未取得のためmain更新できません", true);
            self.log("main更新失敗: 監視データ未取得");
            return;
        };

        if let Some(reason) = self.move_main_action_block_reason(snapshot) {
            self.set_copy_feedback(reason, true);
            self.log(format!("main更新失敗: {reason}"));
            return;
        }

        let Some(repo_path) = self.selected_repo_path.clone() else {
            let message = "プロジェクト未選択のためmain更新できません".to_string();
            self.last_error = Some(message.clone());
            self.set_copy_feedback(message.clone(), true);
            self.log(format!("main更新失敗: {message}"));
            return;
        };

        self.app_status = AppStatus::Updating;
        match git::set_main_branch_to_head(&repo_path) {
            Ok(short_id) => {
                let message = format!("main更新完了: {short_id}");
                self.last_error = None;
                self.set_copy_feedback(message.clone(), false);
                self.log(message);
            }
            Err(err) => {
                let message = format!("main更新失敗: {err}");
                self.last_error = Some(message.clone());
                self.set_copy_feedback(message.clone(), true);
                self.log(message);
            }
        }

        self.monitor.send(MonitorCommand::ManualRefresh);
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

    fn save_external_selected_repo_with_log(&mut self, quiet: bool) {
        match config::save_external_selected_repo_path(self.selected_repo_path.as_deref(), true) {
            Ok(result) => {
                let path_info = result.path_info;
                let path = path_info.path.clone();
                self.external_repo_force_fallback = path_info.using_fallback;
                self.external_repo_path_info = Some(path_info);
                self.last_external_repo_file_value = Some(
                    self.selected_repo_path
                        .as_ref()
                        .map(|selected| selected.to_string_lossy().to_string()),
                );
                self.last_external_repo_poll_path = Some(path.clone());
                self.last_external_repo_modified_at =
                    Some(fs::metadata(&path).ok().and_then(|metadata| metadata.modified().ok()));
                self.last_external_repo_read_error = None;

                if !quiet && result.changed {
                    self.log(format!("外部選択保存: {}", path.display()));
                }
            }
            Err(err) => {
                self.log(format!("外部選択保存失敗: {err}"));
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
        self.poll_external_selected_repo_path(false);
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
        self.render_commit_confirm_dialog(ctx);
        self.render_revert_confirm_dialog(ctx);
        self.render_main_head_confirm_dialog(ctx);
        self.render_commit_message_dialog(ctx);
        self.project_change_dialog_was_open = self.show_project_change_dialog;

        let _ = self.logs.len();
        ctx.request_repaint_after(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::CodexRollbackBridgeApp;
    use eframe::egui::Color32;
    use std::time::{Duration, Instant};

    #[test]
    fn table_row_fill_color_prioritizes_selected() {
        assert_eq!(
            CodexRollbackBridgeApp::table_row_fill_color(true, true, true),
            Color32::from_rgb(236, 236, 236)
        );
    }

    #[test]
    fn table_row_fill_color_sets_head_to_red() {
        assert_eq!(
            CodexRollbackBridgeApp::table_row_fill_color(false, true, true),
            Color32::from_rgb(255, 224, 224)
        );
        assert_eq!(
            CodexRollbackBridgeApp::table_row_fill_color(false, false, true),
            Color32::from_rgb(255, 224, 224)
        );
    }

    #[test]
    fn table_row_fill_color_sets_master_to_green() {
        assert_eq!(
            CodexRollbackBridgeApp::table_row_fill_color(false, true, false),
            Color32::from_rgb(228, 245, 228)
        );
    }

    #[test]
    fn table_row_fill_color_sets_normal_to_white() {
        assert_eq!(
            CodexRollbackBridgeApp::table_row_fill_color(false, false, false),
            Color32::from_rgb(252, 252, 252)
        );
    }

    #[test]
    fn normalize_message_newlines_converts_crlf_and_cr_to_lf() {
        let message = "subject\r\n\r\n- 何を: a\r- 何を: b\n".to_string();
        assert_eq!(
            CodexRollbackBridgeApp::normalize_message_newlines(message),
            "subject\n\n- 何を: a\n- 何を: b\n"
        );
    }

    #[test]
    fn commit_display_row_count_keeps_minimum_visible_rows() {
        assert_eq!(CodexRollbackBridgeApp::commit_display_row_count(0, 10), 10);
        assert_eq!(CodexRollbackBridgeApp::commit_display_row_count(8, 10), 10);
        assert_eq!(CodexRollbackBridgeApp::commit_display_row_count(10, 10), 10);
    }

    #[test]
    fn commit_display_row_count_caps_to_fifty() {
        assert_eq!(CodexRollbackBridgeApp::commit_display_row_count(11, 10), 11);
        assert_eq!(CodexRollbackBridgeApp::commit_display_row_count(50, 10), 50);
        assert_eq!(CodexRollbackBridgeApp::commit_display_row_count(60, 10), 50);
    }

    #[test]
    fn revert_confirm_state_counts_down_before_deadline() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(CodexRollbackBridgeApp::REVERT_CONFIRM_COUNTDOWN_SECONDS);
        assert_eq!(
            CodexRollbackBridgeApp::revert_confirm_state_from_deadline(deadline, now),
            ("5".to_string(), false)
        );
    }

    #[test]
    fn revert_confirm_state_shows_zero_before_ok() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(CodexRollbackBridgeApp::REVERT_CONFIRM_COUNTDOWN_SECONDS);
        assert_eq!(
            CodexRollbackBridgeApp::revert_confirm_state_from_deadline(deadline, deadline),
            ("0".to_string(), false)
        );
    }

    #[test]
    fn revert_confirm_state_enables_confirmation_after_zero() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(CodexRollbackBridgeApp::REVERT_CONFIRM_COUNTDOWN_SECONDS);
        let ready_time = deadline + Duration::from_millis(CodexRollbackBridgeApp::REVERT_CONFIRM_ZERO_DISPLAY_MILLIS + 1);
        assert_eq!(
            CodexRollbackBridgeApp::revert_confirm_state_from_deadline(deadline, ready_time),
            ("OK".to_string(), true)
        );
    }
}
