use crate::config;
use crate::git;
use crate::material;
use crate::models::{AppStatus, RepoCandidate, RepoSnapshot, ScanScope, Settings};
use crate::monitor::{MonitorCommand, MonitorController, MonitorEvent};
use eframe::egui::{self, Color32, RichText, ScrollArea};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct CodexRollbackBridgeApp {
    project_root: PathBuf,
    settings: Settings,
    app_status: AppStatus,
    git_available: bool,
    root_folder_input: String,
    direct_repo_input: String,
    scan_scope: ScanScope,
    scan_results: Vec<RepoCandidate>,
    selected_scan_index: Option<usize>,
    selected_repo_path: Option<PathBuf>,
    snapshot: Option<RepoSnapshot>,
    selected_target_commit: Option<String>,
    material_text: String,
    material_json: String,
    material_kind: String,
    last_error: Option<String>,
    logs: Vec<String>,
    monitor: MonitorController,
    config_contract: String,
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
            direct_repo_input: settings
                .selected_repo_path
                .clone()
                .unwrap_or(default_project_root_text),
            scan_scope: settings.scan_scope,
            scan_results: Vec::new(),
            selected_scan_index: None,
            selected_repo_path,
            snapshot: None,
            selected_target_commit: None,
            material_text: String::new(),
            material_json: String::new(),
            material_kind: String::new(),
            last_error: None,
            logs: load_result.logs,
            monitor: MonitorController::new(settings.update_interval_sec),
            config_contract: config::config_contract_line(),
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
                    let decorated_message =
                        format!("更新失敗({consecutive_failures}回連続): {message}");
                    self.last_error = Some(decorated_message.clone());
                    self.log(decorated_message);
                }
            }
        }
    }

    fn render_top_panel(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Codex Rollback Bridge");
            ui.separator();
            ui.colored_label(
                self.status_color(),
                RichText::new(format!("状態: {}", self.app_status.label())).strong(),
            );
        });

        if let Some(error) = &self.last_error {
            ui.colored_label(Color32::RED, error);
        }
    }

    fn render_project_selection(&mut self, ui: &mut egui::Ui) {
        ui.heading("プロジェクト選択");
        ui.label("ルートフォルダ配下をスキャンして Git リポジトリ候補を表示します。");

        ui.horizontal(|ui| {
            ui.label("ルート:");
            ui.text_edit_singleline(&mut self.root_folder_input);
            if ui.button("ワークスペース").clicked() {
                self.root_folder_input = self.project_root.to_string_lossy().to_string();
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

        if self.scan_results.is_empty() {
            ui.label("候補なし");
            return;
        }

        ui.separator();
        ui.label("候補プロジェクト一覧");
        ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
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

        if ui.button("選択確定").clicked() {
            self.confirm_selected_repo();
        }

        ui.separator();
        ui.label("直接選択");
        ui.horizontal(|ui| {
            ui.label("リポジトリ:");
            ui.text_edit_singleline(&mut self.direct_repo_input);
            if ui.button("このプロジェクト").clicked() {
                self.direct_repo_input = self.project_root.to_string_lossy().to_string();
            }
        });
        if ui.button("このパスを選択").clicked() {
            self.confirm_direct_repo_input();
        }
    }

    fn render_main_screen(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("プロジェクト再選択").clicked() {
                self.clear_selected_repo();
            }
            if ui.button("手動更新").clicked() {
                self.app_status = AppStatus::Updating;
                self.monitor.send(MonitorCommand::ManualRefresh);
            }
        });

        if let Some(repo_path) = &self.selected_repo_path {
            ui.label(format!("監視対象: {}", repo_path.display()));
        }

        ui.horizontal(|ui| {
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
        });

        ui.separator();
        if let Some(snapshot) = &self.snapshot {
            ui.label(format!("現在ブランチ: {}", snapshot.current_branch));
            ui.label(format!("HEAD: {}", snapshot.head_full_id));
            ui.label(format!("HEAD message: {}", snapshot.head_message));
            ui.label(format!("HEAD datetime: {}", snapshot.head_datetime));
            ui.label(format!("dirty: {}", snapshot.dirty));
            ui.label(format!("head_changed: {}", snapshot.head_changed));
            ui.label(format!(
                "consecutive_update_failures: {}",
                snapshot.consecutive_update_failures
            ));
            ui.separator();

            ui.label("コミット一覧（最大50）");
            ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                let mut clicked_target: Option<String> = None;
                for commit in &snapshot.recent_commits {
                    let selected = self
                        .selected_target_commit
                        .as_deref()
                        .map(|id| id == commit.full_id)
                        .unwrap_or(false);
                    let row = format!(
                        "{} | {} | {}",
                        commit.datetime, commit.short_id, commit.message
                    );
                    if ui.selectable_label(selected, row).clicked() {
                        clicked_target = Some(commit.full_id.clone());
                    }
                }
                if let Some(target) = clicked_target {
                    self.selected_target_commit = Some(target);
                }
            });
        } else {
            ui.label("監視データ未取得");
        }

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("最小素材生成").clicked() {
                self.generate_material(false);
            }
            if ui.button("詳細素材生成").clicked() {
                self.generate_material(true);
            }
        });

        if let Some(target) = &self.selected_target_commit {
            ui.label(format!("target_full_id: {target}"));
        } else {
            ui.colored_label(Color32::from_rgb(128, 96, 0), "ターゲットコミット未選択");
        }

        if !self.material_kind.is_empty() {
            ui.separator();
            ui.label(format!("生成種別: {}", self.material_kind));
            ui.label(format!(
                "UTF-8 bytes (text/json): {}/{}",
                self.material_text.len(),
                self.material_json.len()
            ));
            ui.horizontal(|ui| {
                if ui.button("テキストをコピー").clicked() {
                    ui.ctx().copy_text(self.material_text.clone());
                }
                if ui.button("JSONをコピー").clicked() {
                    ui.ctx().copy_text(self.material_json.clone());
                }
            });
            ui.collapsing("人間向けテキスト", |ui| {
                ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                    ui.code(&self.material_text);
                });
            });
            ui.collapsing("JSON", |ui| {
                ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                    ui.code(&self.material_json);
                });
            });
        }
    }

    fn render_logs(&self, ui: &mut egui::Ui) {
        ui.separator();
        ui.label("ログ");
        ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
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

    fn confirm_selected_repo(&mut self) {
        let Some(index) = self.selected_scan_index else {
            self.last_error = Some("候補プロジェクト未選択".to_string());
            return;
        };
        let Some(candidate) = self.scan_results.get(index) else {
            self.last_error = Some("候補プロジェクトの参照に失敗しました".to_string());
            return;
        };
        self.set_selected_repo(candidate.path.clone());
    }

    fn clear_selected_repo(&mut self) {
        self.selected_repo_path = None;
        self.selected_target_commit = None;
        self.snapshot = None;
        self.material_text.clear();
        self.material_json.clear();
        self.material_kind.clear();
        self.app_status = AppStatus::ProjectUnselected;
        self.last_error = None;
        self.save_settings_with_log();
    }

    fn confirm_direct_repo_input(&mut self) {
        let repo_path_text = self.direct_repo_input.trim();
        if repo_path_text.is_empty() {
            self.last_error = Some("リポジトリパスが未入力です".to_string());
            return;
        }

        let repo_path = PathBuf::from(repo_path_text);
        if !repo_path.is_dir() {
            self.last_error = Some(format!(
                "リポジトリパスが不正です: {}",
                repo_path.to_string_lossy()
            ));
            return;
        }

        if !git::is_git_repo(&repo_path) {
            self.last_error = Some(format!(
                "Gitリポジトリではありません (.git が必要): {}",
                repo_path.to_string_lossy()
            ));
            self.log("直接選択失敗: Gitリポジトリ判定に失敗");
            return;
        }

        self.set_selected_repo(repo_path);
    }

    fn set_selected_repo(&mut self, repo_path: PathBuf) {
        self.selected_repo_path = Some(repo_path.clone());
        self.direct_repo_input = repo_path.to_string_lossy().to_string();
        self.snapshot = None;
        self.selected_target_commit = None;
        self.material_text.clear();
        self.material_json.clear();
        self.material_kind.clear();
        self.app_status = AppStatus::Updating;
        self.last_error = None;

        self.monitor.send(MonitorCommand::SetRepo(repo_path));
        self.save_settings_with_log();
    }

    fn generate_material(&mut self, detailed: bool) {
        let Some(snapshot) = self.snapshot.clone() else {
            self.last_error = Some("監視データ未取得".to_string());
            return;
        };

        let Some(target_full_id) = self.selected_target_commit.clone() else {
            self.last_error = Some("ターゲットコミット未選択".to_string());
            self.log("素材生成失敗: ターゲットコミット未選択");
            return;
        };

        let generated = if detailed {
            material::generate_detailed(Path::new(&snapshot.repo_path), &snapshot, &target_full_id)
        } else {
            material::generate_minimal(&snapshot, &target_full_id)
        };

        match generated {
            Ok(output) => {
                self.material_text = output.text;
                self.material_json = output.json;
                self.material_kind = if detailed {
                    if output.omitted_due_to_size {
                        "詳細素材（容量超過で省略）".to_string()
                    } else {
                        "詳細素材".to_string()
                    }
                } else {
                    "最小素材".to_string()
                };
                self.last_error = None;
                self.log(format!("素材生成成功: {}", self.material_kind));
            }
            Err(err) => {
                self.last_error = Some(err.clone());
                self.log(format!("素材生成失敗: {err}"));
            }
        }
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
            if self.selected_repo_path.is_some() {
                self.render_main_screen(ui);
            } else {
                self.render_project_selection(ui);
            }
            self.render_logs(ui);
        });

        ctx.request_repaint_after(Duration::from_millis(200));
    }
}
