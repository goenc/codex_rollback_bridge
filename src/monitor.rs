use crate::git;
use crate::models::{CommitInfo, RepoSnapshot};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

pub enum MonitorCommand {
    SetRepo(PathBuf),
    SetIntervalSeconds(u64),
    ManualRefresh,
    Stop,
}

pub enum MonitorEvent {
    Snapshot(RepoSnapshot),
    Error {
        message: String,
        consecutive_failures: u32,
    },
}

pub struct MonitorController {
    command_tx: Sender<MonitorCommand>,
    event_rx: Receiver<MonitorEvent>,
}

impl MonitorController {
    pub fn new(initial_interval_sec: u64) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        thread::spawn(move || worker_loop(command_rx, event_tx, initial_interval_sec));

        Self {
            command_tx,
            event_rx,
        }
    }

    pub fn send(&self, command: MonitorCommand) {
        let _ = self.command_tx.send(command);
    }

    pub fn try_recv(&self) -> Option<MonitorEvent> {
        self.event_rx.try_recv().ok()
    }
}

impl Drop for MonitorController {
    fn drop(&mut self) {
        let _ = self.command_tx.send(MonitorCommand::Stop);
    }
}

struct WorkerState {
    repo_path: Option<PathBuf>,
    interval: Duration,
    previous_head: Option<String>,
    previous_commits: Vec<CommitInfo>,
    consecutive_failures: u32,
}

fn worker_loop(
    command_rx: Receiver<MonitorCommand>,
    event_tx: Sender<MonitorEvent>,
    initial_interval_sec: u64,
) {
    let mut state = WorkerState {
        repo_path: None,
        interval: interval_from_seconds(initial_interval_sec),
        previous_head: None,
        previous_commits: Vec::new(),
        consecutive_failures: 0,
    };

    loop {
        match command_rx.recv_timeout(state.interval) {
            Ok(command) => match command {
                MonitorCommand::SetRepo(repo_path) => {
                    state.repo_path = Some(repo_path);
                    state.previous_head = None;
                    state.previous_commits.clear();
                    state.consecutive_failures = 0;
                    refresh_once(&mut state, &event_tx);
                }
                MonitorCommand::SetIntervalSeconds(seconds) => {
                    state.interval = interval_from_seconds(seconds);
                }
                MonitorCommand::ManualRefresh => {
                    // Manual refresh is expected to recover a stale table view immediately.
                    state.previous_head = None;
                    state.previous_commits.clear();
                    refresh_once(&mut state, &event_tx);
                }
                MonitorCommand::Stop => break,
            },
            Err(RecvTimeoutError::Timeout) => {
                refresh_once(&mut state, &event_tx);
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn refresh_once(state: &mut WorkerState, event_tx: &Sender<MonitorEvent>) {
    let Some(repo_path) = state.repo_path.clone() else {
        return;
    };

    match git::fetch_snapshot(&repo_path, &state.previous_head, &state.previous_commits) {
        Ok(mut snapshot) => {
            state.consecutive_failures = 0;
            state.previous_head = Some(snapshot.head_full_id.clone());
            state.previous_commits = snapshot.recent_commits.clone();
            snapshot.consecutive_update_failures = 0;
            let _ = event_tx.send(MonitorEvent::Snapshot(snapshot));
        }
        Err(message) => {
            state.consecutive_failures = state.consecutive_failures.saturating_add(1);
            let _ = event_tx.send(MonitorEvent::Error {
                message,
                consecutive_failures: state.consecutive_failures,
            });
        }
    }
}

fn interval_from_seconds(seconds: u64) -> Duration {
    Duration::from_secs(seconds.clamp(1, 3600))
}
