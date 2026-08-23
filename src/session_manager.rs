use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tokio::sync::{broadcast, mpsc};

/// Max scrollback buffer size in bytes (2MB of encoded data)
const SCROLLBACK_MAX_BYTES: usize = 2 * 1024 * 1024;

/// Broadcast channel capacity — slow clients that fall behind will get Lagged error
const BROADCAST_CAPACITY: usize = 512;

use crate::acp::kiro_process::KiroProcess;
use crate::acp::process::AcpProcess;
use crate::pty_bridge::PtyHandle;

#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionMeta {
    Running,
    Done,
    Blocked,
    Idle,
}

impl Default for SessionMeta {
    fn default() -> Self {
        Self::Running
    }
}

impl std::fmt::Display for SessionMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionMeta::Running => write!(f, "running"),
            SessionMeta::Done => write!(f, "done"),
            SessionMeta::Blocked => write!(f, "blocked"),
            SessionMeta::Idle => write!(f, "idle"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionType {
    Tmux,
    Claude,
    Kiro,
    Codex,
}

impl std::fmt::Display for SessionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionType::Tmux => write!(f, "tmux"),
            SessionType::Claude => write!(f, "claude"),
            SessionType::Kiro => write!(f, "kiro"),
            SessionType::Codex => write!(f, "codex"),
        }
    }
}

/// Input commands from WS clients to the session process
pub enum SessionInput {
    /// PTY: raw bytes (base64-decoded by WS handler)
    PtyData(Vec<u8>),
    /// PTY: resize
    PtyResize(u16, u16),
    /// ACP/Kiro: prompt text
    Prompt(String),
    /// ACP/Kiro: cancel/kill
    Cancel,
}

pub struct Session {
    pub id: String,
    pub name: String,
    pub session_type: SessionType,
    pub cols: u16,
    pub rows: u16,
    pub work_dir: String,
    pub owner_id: String,
    pub description: String,
    pub status: SessionMeta,
    /// Broadcast channel: fan-out task writes, all WS clients subscribe
    event_tx: broadcast::Sender<String>,
    /// Input channel: any WS client writes, fan-out task forwards to process
    input_tx: mpsc::Sender<SessionInput>,
    /// Git worktree path for ACP sessions (cleaned up on delete)
    worktree_path: Option<PathBuf>,
    /// PTY child PID kept for /proc lookup (PTY sessions only)
    pty_pid: Option<u32>,
    /// Underlying tmux session name (if backed by tmux)
    tmux_session_name: Option<String>,
    /// Active recording: (file_path, start_snapshot_lines)
    recording: Option<(PathBuf, Vec<String>)>,
    /// Output history for replay on reconnect (base64 for PTY, JSON for ACP/Kiro)
    scrollback: VecDeque<String>,
    scrollback_bytes: usize,
}

pub struct SessionManager {
    sessions: Mutex<HashMap<String, Session>>,
}

#[derive(serde::Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub session_type: SessionType,
    pub cols: u16,
    pub rows: u16,
    pub work_dir: String,
    pub description: String,
    pub status: SessionMeta,
}

pub struct RecordingStart {
    pub file_path: PathBuf,
    pub work_dir: String,
    pub session_name: String,
    pub tmux_name: String,
}

pub struct RecordingStop {
    pub file_path: PathBuf,
    pub new_line_count: usize,
}

// ── Git worktree helpers ──

/// Check if a directory is inside a git repo
fn is_git_repo(dir: &Path) -> bool {
    std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Create a git worktree. Returns the worktree path on success.
fn create_worktree(repo_dir: &Path, session_id: &str) -> Result<PathBuf, String> {
    let worktrees_dir = repo_dir.join(".zeromux-worktrees");
    std::fs::create_dir_all(&worktrees_dir)
        .map_err(|e| format!("Failed to create worktrees dir: {}", e))?;

    let short_id = &session_id[..8.min(session_id.len())];
    let wt_path = worktrees_dir.join(short_id);

    let output = std::process::Command::new("git")
        .args(["worktree", "add", "--detach"])
        .arg(&wt_path)
        .current_dir(repo_dir)
        .output()
        .map_err(|e| format!("Failed to run git worktree add: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git worktree add failed: {}", stderr));
    }

    tracing::info!("Created git worktree at {}", wt_path.display());
    Ok(wt_path)
}

/// Remove a git worktree
fn remove_worktree(repo_dir: &Path, wt_path: &Path) {
    let result = std::process::Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(wt_path)
        .current_dir(repo_dir)
        .output();

    match result {
        Ok(output) if output.status.success() => {
            tracing::info!("Removed git worktree at {}", wt_path.display());
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("git worktree remove failed: {}", stderr);
            let _ = std::fs::remove_dir_all(wt_path);
        }
        Err(e) => {
            tracing::warn!("Failed to run git worktree remove: {}", e);
            let _ = std::fs::remove_dir_all(wt_path);
        }
    }
}

/// Resolve the effective work directory: create a worktree if inside a git repo,
/// otherwise return the original path.
fn resolve_work_dir(work_dir: &str, session_id: &str) -> (PathBuf, Option<PathBuf>) {
    let base = if work_dir == "." {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        PathBuf::from(work_dir)
    };

    if is_git_repo(&base) {
        match create_worktree(&base, session_id) {
            Ok(wt_path) => (wt_path.clone(), Some(wt_path)),
            Err(e) => {
                tracing::warn!("Worktree creation failed, using base dir: {}", e);
                (base, None)
            }
        }
    } else {
        (base, None)
    }
}

/// Check if tmux binary is available
fn has_tmux_binary() -> bool {
    std::process::Command::new("tmux")
        .arg("-V")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

impl SessionManager {
    pub fn new() -> Self {
        if !has_tmux_binary() {
            eprintln!("WARNING: tmux not found. Sessions will not survive ZeroMux restart. Install: apt install tmux");
        }
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn create_pty_session(
        &self,
        name: String,
        shell: &str,
        work_dir: &str,
        cols: u16,
        rows: u16,
        owner_id: &str,
        tmux_target: Option<&str>,
    ) -> Result<String, String> {
        // When attaching to an existing tmux session, the caller usually doesn't
        // know its directory — derive it from the session's active pane so the
        // stored work_dir (used by the file browser / md docs) points at the
        // session's real project dir instead of the server's launch dir.
        let tmux_pane_dir: Option<String> = tmux_target.and_then(|target| {
            std::process::Command::new("tmux")
                .args([
                    "display-message",
                    "-p",
                    "-t",
                    target,
                    "-F",
                    "#{pane_current_path}",
                ])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        });

        let cwd = if let Some(ref dir) = tmux_pane_dir {
            Some(dir.as_str())
        } else if work_dir.is_empty() || work_dir == "." {
            None
        } else {
            Some(work_dir)
        };

        let id = uuid::Uuid::new_v4().to_string();
        let has_tmux = has_tmux_binary();

        // Determine command and tmux session name
        let tmux_session_name: Option<String>;
        let cmd: String;
        let args_owned: Vec<String>;

        if let Some(target) = tmux_target {
            // Attach to existing tmux session
            configure_tmux_scroll(target);
            cmd = "tmux".to_string();
            args_owned = vec!["attach".to_string(), "-t".to_string(), target.to_string()];
            tmux_session_name = Some(target.to_string());
        } else if has_tmux {
            // Use a fixed name based on session name; create-or-attach
            let tmux_name = format!("zmux-{}", name.replace(' ', "-").to_lowercase());
            // Check if session already exists
            let exists = std::process::Command::new("tmux")
                .args(["has-session", "-t", &tmux_name])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);

            if !exists {
                let mut create_cmd = std::process::Command::new("tmux");
                create_cmd.args([
                    "new-session",
                    "-d",
                    "-s",
                    &tmux_name,
                    "-x",
                    &cols.to_string(),
                    "-y",
                    &rows.to_string(),
                ]);
                if let Some(dir) = cwd {
                    create_cmd.args(["-c", dir]);
                }
                create_cmd.arg(shell);
                let output = create_cmd
                    .output()
                    .map_err(|e| format!("Failed to create tmux session: {}", e))?;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(format!("tmux new-session failed: {}", stderr));
                }
                let _ = std::process::Command::new("tmux")
                    .args(["set-option", "-t", &tmux_name, "destroy-unattached", "off"])
                    .output();
                let _ = std::process::Command::new("tmux")
                    .args(["set-option", "-t", &tmux_name, "exit-empty", "off"])
                    .output();
                // Size the window to the LARGEST attached client rather than the
                // smallest (tmux 3.x default is "latest", which lets a web client
                // and an ssh client attached to the same session fight over the
                // window size and truncate each other's output). With "largest"
                // the big client renders fully; smaller clients scroll to view.
                let _ = std::process::Command::new("tmux")
                    .args(["set-option", "-t", &tmux_name, "window-size", "largest"])
                    .output();
                let _ = std::process::Command::new("tmux")
                    .args([
                        "set-window-option",
                        "-t",
                        &tmux_name,
                        "aggressive-resize",
                        "on",
                    ])
                    .output();
            }
            configure_tmux_scroll(&tmux_name);
            cmd = "tmux".to_string();
            args_owned = vec!["attach".to_string(), "-t".to_string(), tmux_name.clone()];
            tmux_session_name = Some(tmux_name);
        } else {
            cmd = shell.to_string();
            args_owned = vec![];
            tmux_session_name = None;
        };

        let args_refs: Vec<&str> = args_owned.iter().map(|s| s.as_str()).collect();
        let (pty, mut output_rx) = PtyHandle::spawn(&cmd, &args_refs, &[], cols, rows, cwd)
            .map_err(|e| format!("Failed to spawn PTY: {}", e))?;

        let effective_dir = match cwd {
            Some(dir) => dir.to_string(),
            None => std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        };

        let (event_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (input_tx, mut input_rx) = mpsc::channel::<SessionInput>(64);

        let pid = pty.pid();
        let event_tx_clone = event_tx.clone();
        let sid = id.clone();
        let _is_tmux_backed = tmux_session_name.is_some();
        let tmux_session_name_clone = tmux_session_name.clone();

        // Spawn fan-out task: owns the PtyHandle, reads output, handles input
        tokio::spawn(async move {
            let mut pty = pty; // move pty into task
            loop {
                tokio::select! {
                    data = output_rx.recv() => {
                        match data {
                            Some(bytes) => {
                                let b64 = base64::Engine::encode(
                                    &base64::engine::general_purpose::STANDARD, &bytes);
                                let _ = event_tx_clone.send(b64);
                            }
                            None => {
                                tracing::info!("PTY output closed for session {}", sid);
                                break;
                            }
                        }
                    }
                    input = input_rx.recv() => {
                        match input {
                            Some(SessionInput::PtyData(bytes)) => {
                                let _ = pty.write_input(&bytes);
                            }
                            Some(SessionInput::PtyResize(cols, rows)) => {
                                let _ = pty.resize(cols, rows);
                            }
                            None => break,
                            _ => {}
                        }
                    }
                }
            }
            // If tmux-backed, detach the client first then forget the pty handle
            // to avoid portable_pty killing the tmux session on drop.
            if let Some(ref name) = tmux_session_name_clone {
                let _ = std::process::Command::new("tmux")
                    .args(["detach-client", "-s", name])
                    .output();
            }
            std::mem::forget(pty);
        });

        let session = Session {
            id: id.clone(),
            name,
            session_type: SessionType::Tmux,
            cols,
            rows,
            work_dir: effective_dir,
            owner_id: owner_id.to_string(),
            description: String::new(),
            status: SessionMeta::Running,
            event_tx,
            input_tx,
            worktree_path: None,
            pty_pid: pid,
            tmux_session_name,
            recording: None,
            scrollback: VecDeque::new(),
            scrollback_bytes: 0,
        };

        self.sessions.lock().unwrap().insert(id.clone(), session);
        Ok(id)
    }

    pub async fn create_acp_session(
        &self,
        name: String,
        claude_path: &str,
        work_dir: &str,
        cols: u16,
        rows: u16,
        owner_id: &str,
    ) -> Result<String, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let (effective_dir, worktree_path) = resolve_work_dir(work_dir, &id);

        let process = AcpProcess::spawn(claude_path, effective_dir.to_str().unwrap_or("."))
            .await
            .map_err(|e| {
                if let Some(wt) = &worktree_path {
                    let base = PathBuf::from(work_dir);
                    remove_worktree(&base, wt);
                }
                format!("Failed to spawn Claude: {}", e)
            })?;

        let (event_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (input_tx, input_rx) = mpsc::channel::<SessionInput>(64);

        let event_tx_clone = event_tx.clone();
        let sid = id.clone();

        // Spawn fan-out task for ACP process
        spawn_acp_fanout(sid, process, event_tx_clone, input_rx);

        let session = Session {
            id: id.clone(),
            name,
            session_type: SessionType::Claude,
            cols,
            rows,
            work_dir: effective_dir.to_string_lossy().to_string(),
            owner_id: owner_id.to_string(),
            description: String::new(),
            status: SessionMeta::Running,
            event_tx,
            input_tx,
            worktree_path,
            pty_pid: None,
            tmux_session_name: None,
            recording: None,
            scrollback: VecDeque::new(),
            scrollback_bytes: 0,
        };

        self.sessions.lock().unwrap().insert(id.clone(), session);
        Ok(id)
    }

    pub async fn create_kiro_session(
        &self,
        name: String,
        kiro_path: &str,
        work_dir: &str,
        cols: u16,
        rows: u16,
        owner_id: &str,
    ) -> Result<String, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let (effective_dir, worktree_path) = resolve_work_dir(work_dir, &id);

        let process = KiroProcess::spawn(kiro_path, effective_dir.to_str().unwrap_or("."))
            .await
            .map_err(|e| {
                if let Some(wt) = &worktree_path {
                    let base = PathBuf::from(work_dir);
                    remove_worktree(&base, wt);
                }
                format!("Failed to spawn Kiro: {}", e)
            })?;

        let (event_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (input_tx, input_rx) = mpsc::channel::<SessionInput>(64);

        let event_tx_clone = event_tx.clone();
        let sid = id.clone();

        // Spawn fan-out task for Kiro process
        spawn_kiro_fanout(sid, process, event_tx_clone, input_rx);

        let session = Session {
            id: id.clone(),
            name,
            session_type: SessionType::Kiro,
            cols,
            rows,
            work_dir: effective_dir.to_string_lossy().to_string(),
            owner_id: owner_id.to_string(),
            description: String::new(),
            status: SessionMeta::Running,
            event_tx,
            input_tx,
            worktree_path,
            pty_pid: None,
            tmux_session_name: None,
            recording: None,
            scrollback: VecDeque::new(),
            scrollback_bytes: 0,
        };

        self.sessions.lock().unwrap().insert(id.clone(), session);
        Ok(id)
    }

    pub async fn create_codex_session(
        &self,
        name: String,
        codex_path: &str,
        work_dir: &str,
        cols: u16,
        rows: u16,
        owner_id: &str,
    ) -> Result<String, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let (effective_dir, worktree_path) = resolve_work_dir(work_dir, &id);

        let process = crate::acp::codex_process::CodexProcess::spawn(
            codex_path,
            effective_dir.to_str().unwrap_or("."),
        )
        .await
        .map_err(|e| {
            if let Some(wt) = &worktree_path {
                let base = std::path::PathBuf::from(work_dir);
                remove_worktree(&base, wt);
            }
            format!("Failed to spawn Codex: {}", e)
        })?;

        let (event_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (input_tx, input_rx) = mpsc::channel::<SessionInput>(64);

        let event_tx_clone = event_tx.clone();
        let sid = id.clone();

        spawn_codex_fanout(sid, process, event_tx_clone, input_rx);

        let session = Session {
            id: id.clone(),
            name,
            session_type: SessionType::Codex,
            cols,
            rows,
            work_dir: effective_dir.to_string_lossy().to_string(),
            owner_id: owner_id.to_string(),
            description: String::new(),
            status: SessionMeta::Running,
            event_tx,
            input_tx,
            worktree_path,
            pty_pid: None,
            tmux_session_name: None,
            recording: None,
            scrollback: VecDeque::new(),
            scrollback_bytes: 0,
        };

        self.sessions.lock().unwrap().insert(id.clone(), session);
        Ok(id)
    }

    /// List sessions, optionally filtered by owner. Pass None for all (admin).
    pub fn list_sessions(&self, owner_filter: Option<&str>) -> Vec<SessionInfo> {
        self.sessions
            .lock()
            .unwrap()
            .values()
            .filter(|s| owner_filter.map(|uid| s.owner_id == uid).unwrap_or(true))
            .map(|s| SessionInfo {
                id: s.id.clone(),
                name: s.name.clone(),
                session_type: s.session_type,
                cols: s.cols,
                rows: s.rows,
                work_dir: s.work_dir.clone(),
                description: s.description.clone(),
                status: s.status,
            })
            .collect()
    }

    /// Check if a user owns a session
    pub fn is_owner(&self, session_id: &str, user_id: &str) -> bool {
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .map(|s| s.owner_id == user_id)
            .unwrap_or(false)
    }

    /// Remove session from ZeroMux. If backed by tmux, the tmux session is kept alive (detach).
    /// Use `kill_session` to also terminate the tmux session.
    pub fn remove_session(&self, id: &str) -> bool {
        let removed = self.sessions.lock().unwrap().remove(id);
        if let Some(session) = removed {
            // Flush recording if active (capture remaining output)
            if let (Some(ref tmux_name), Some((ref file_path, ref baseline))) =
                (&session.tmux_session_name, &session.recording)
            {
                let current = capture_pane_content(tmux_name);
                let skip = find_divergence_point(baseline, &current);
                let new_lines = &current[skip..];
                if !new_lines.is_empty() {
                    if let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(file_path)
                    {
                        use std::io::Write;
                        let _ = writeln!(file, "{}", new_lines.join("\n"));
                    }
                }
            }
            // Dropping session closes event_tx + input_tx → fan-out task exits
            // tmux-backed sessions survive (detach behavior)
            if session.tmux_session_name.is_some() {
                tracing::info!(
                    "Detached from tmux session {:?} (session {})",
                    session.tmux_session_name,
                    id
                );
            }
            if let Some(wt_path) = &session.worktree_path {
                if let Some(worktrees_dir) = wt_path.parent() {
                    if let Some(repo_dir) = worktrees_dir.parent() {
                        remove_worktree(repo_dir, wt_path);
                    }
                }
            }
            true
        } else {
            false
        }
    }

    /// Kill session: remove from ZeroMux AND kill the underlying tmux session
    pub fn kill_session(&self, id: &str) -> bool {
        let removed = self.sessions.lock().unwrap().remove(id);
        if let Some(session) = removed {
            if let Some(ref tmux_name) = session.tmux_session_name {
                let _ = std::process::Command::new("tmux")
                    .args(["kill-session", "-t", tmux_name])
                    .output();
                tracing::info!("Killed tmux session {} (session {})", tmux_name, id);
            }
            if let Some(wt_path) = &session.worktree_path {
                if let Some(worktrees_dir) = wt_path.parent() {
                    if let Some(repo_dir) = worktrees_dir.parent() {
                        remove_worktree(repo_dir, wt_path);
                    }
                }
            }
            true
        } else {
            false
        }
    }

    /// Get the tmux session name for a given session
    pub fn tmux_session_name(&self, id: &str) -> Option<String> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .and_then(|s| s.tmux_session_name.clone())
    }

    // ── Broadcast API: subscribe to session events ──

    /// Subscribe to a session's event broadcast. Returns None if session not found.
    pub fn subscribe(&self, id: &str) -> Option<broadcast::Receiver<String>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .map(|s| s.event_tx.subscribe())
    }

    /// Get the input sender for a session. Returns None if session not found.
    pub fn input_tx(&self, id: &str) -> Option<mpsc::Sender<SessionInput>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .map(|s| s.input_tx.clone())
    }

    // (PTY write/resize now handled via input_tx → fan-out task)

    /// Update session metadata (description, status)
    pub fn update_session_meta(
        &self,
        id: &str,
        description: Option<String>,
        status: Option<SessionMeta>,
    ) -> bool {
        if let Some(session) = self.sessions.lock().unwrap().get_mut(id) {
            if let Some(d) = description {
                session.description = d;
            }
            if let Some(s) = status {
                session.status = s;
            }
            true
        } else {
            false
        }
    }

    /// Get session type for a given id
    pub fn session_type(&self, id: &str) -> Option<SessionType> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .map(|s| s.session_type)
    }

    /// Push output data to the scrollback buffer (base64 for PTY, JSON for ACP/Kiro)
    pub fn push_scrollback(&self, id: &str, data: String) {
        if let Some(s) = self.sessions.lock().unwrap().get_mut(id) {
            let data_len = data.len();
            s.scrollback.push_back(data);
            s.scrollback_bytes += data_len;
            while s.scrollback_bytes > SCROLLBACK_MAX_BYTES && !s.scrollback.is_empty() {
                if let Some(removed) = s.scrollback.pop_front() {
                    s.scrollback_bytes -= removed.len();
                }
            }
        }
    }

    /// Get a clone of the scrollback buffer for replay
    pub fn get_scrollback(&self, id: &str) -> Vec<String> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .map(|s| s.scrollback.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Get work_dir for a session
    pub fn work_dir(&self, id: &str) -> Option<String> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .map(|s| s.work_dir.clone())
    }

    /// Get PTY child PID for a session
    pub fn pty_pid(&self, id: &str) -> Option<u32> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .and_then(|s| s.pty_pid)
    }

    /// Start recording: snapshot current pane content as baseline
    pub fn start_recording(&self, id: &str) -> Result<RecordingStart, String> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions.get_mut(id).ok_or("Session not found")?;

        if session.recording.is_some() {
            return Err("Already recording".to_string());
        }

        let tmux_name = session
            .tmux_session_name
            .clone()
            .ok_or("Not a tmux-backed session")?;
        let work_dir = session.work_dir.clone();
        let session_name = session.name.clone();

        let context_dir = PathBuf::from(&work_dir).join(".zeromux").join("context");
        std::fs::create_dir_all(&context_dir)
            .map_err(|e| format!("Cannot create context dir: {}", e))?;

        let now = chrono_now();
        let safe_name = safe_filename_component(&session_name);
        let mut filename = format!("{}-{}.log", safe_name, now);
        let mut file_path = context_dir.join(&filename);
        if file_path.exists() {
            let suffix = uuid::Uuid::new_v4().to_string().replace('-', "");
            filename = format!("{}-{}-{}.log", safe_name, now, &suffix[..8]);
            file_path = context_dir.join(&filename);
        }
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&file_path)
            .map_err(|e| format!("Cannot create recording file: {}", e))?;

        // Snapshot current pane as baseline (non-blank lines from bottom)
        let baseline = capture_pane_content(&tmux_name);

        session.recording = Some((file_path.clone(), baseline));
        tracing::info!("Started recording session {}", id);
        Ok(RecordingStart {
            file_path,
            work_dir,
            session_name,
            tmux_name,
        })
    }

    /// Stop recording: capture-pane, diff against baseline, append new content to file
    pub fn stop_recording(&self, id: &str) -> Result<Option<RecordingStop>, String> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions.get_mut(id).ok_or("Session not found")?;

        let recording = session.recording.take();
        if let Some((file_path, baseline)) = recording {
            let tmux_name = session
                .tmux_session_name
                .as_ref()
                .ok_or("Not a tmux-backed session")?;

            let current = capture_pane_content(tmux_name);

            // Find where current diverges from baseline
            // The baseline is a suffix of an earlier capture; find how much new content appeared
            let skip = find_divergence_point(&baseline, &current);
            let new_lines = &current[skip..];

            if !new_lines.is_empty() {
                let new_content = new_lines.join("\n");
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&file_path)
                    .map_err(|e| format!("Cannot open file: {}", e))?;
                writeln!(file, "{}", new_content).map_err(|e| format!("Write failed: {}", e))?;
            }

            tracing::info!(
                "Stopped recording session {} ({} new lines)",
                id,
                new_lines.len()
            );
            Ok(Some(RecordingStop {
                file_path,
                new_line_count: new_lines.len(),
            }))
        } else {
            Ok(None)
        }
    }

    /// Get current recording status
    pub fn recording_status(&self, id: &str) -> Option<Option<PathBuf>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .map(|s| s.recording.as_ref().map(|(p, _)| p.clone()))
    }
}

/// Enable scroll-back via mouse for a tmux session: wheel-up enters copy-mode
/// (scrolls history), wheel over a mouse-aware program (claude/codex TUIs)
/// still passes through. history-limit is set globally because tmux only
/// applies it to panes created afterwards.
fn configure_tmux_scroll(tmux_name: &str) {
    let _ = std::process::Command::new("tmux")
        .args(["set-option", "-t", tmux_name, "mouse", "on"])
        .output();
    let _ = std::process::Command::new("tmux")
        .args(["set-option", "-g", "history-limit", "50000"])
        .output();
}

/// Capture current pane content as Vec of non-trailing-blank lines
fn capture_pane_content(tmux_name: &str) -> Vec<String> {
    let output = std::process::Command::new("tmux")
        .args(["capture-pane", "-p", "-J", "-S", "-", "-t", tmux_name])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    // Trim trailing blank lines
    let lines: Vec<String> = output.lines().map(|l| l.to_string()).collect();
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map(|i| i + 1)
        .unwrap_or(0);
    lines[..end].to_vec()
}

fn safe_filename_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_was_dash = false;

    for ch in input.chars() {
        let next = if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            last_was_dash = false;
            Some(ch.to_ascii_lowercase())
        } else {
            if last_was_dash {
                None
            } else {
                last_was_dash = true;
                Some('-')
            }
        };
        if let Some(ch) = next {
            out.push(ch);
        }
    }

    let trimmed = out.trim_matches('-').trim_matches('.');
    if trimmed.is_empty() {
        "session".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Find where `current` diverges from `baseline`.
/// Returns the index in `current` from which new content starts.
fn find_divergence_point(baseline: &[String], current: &[String]) -> usize {
    if baseline.is_empty() {
        return 0;
    }
    // The last non-blank line of baseline should appear somewhere in current.
    // Find the longest matching suffix of baseline in current.
    let bl = baseline.len();
    let cl = current.len();

    // Try to find baseline's last few lines in current as an anchor
    let anchor_size = bl.min(5); // use last 5 lines as anchor
    let anchor = &baseline[bl - anchor_size..];

    // Search for anchor in current
    if cl >= anchor_size {
        for start in (0..=(cl - anchor_size)).rev() {
            if &current[start..start + anchor_size] == anchor {
                return start + anchor_size;
            }
        }
    }
    // Fallback: if baseline is a prefix of current, skip baseline length
    let common = baseline
        .iter()
        .zip(current.iter())
        .take_while(|(a, b)| a == b)
        .count();
    if common > 0 {
        common
    } else {
        0
    }
}

/// Generate timestamp string for filenames: YYYYMMDD-HHmmss
fn chrono_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = secs / 86400;
    let mut y = 1970i64;
    let mut remaining = days as i64;
    loop {
        let diy = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
            366
        } else {
            365
        };
        if remaining < diy {
            break;
        }
        remaining -= diy;
        y += 1;
    }
    let months: [i64; 12] = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut mo = 1;
    for &md in &months {
        if remaining < md {
            break;
        }
        remaining -= md;
        mo += 1;
    }
    let day = remaining + 1;
    let time_secs = secs % 86400;
    let h = time_secs / 3600;
    let m = (time_secs % 3600) / 60;
    let s = time_secs % 60;
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", y, mo, day, h, m, s)
}

// ── Fan-out tasks for ACP/Kiro processes ──

fn spawn_acp_fanout(
    sid: String,
    mut process: AcpProcess,
    event_tx: broadcast::Sender<String>,
    mut input_rx: mpsc::Receiver<SessionInput>,
) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                event = process.event_rx.recv() => {
                    match event {
                        Some(evt) => {
                            let json = match serde_json::to_string(&evt) {
                                Ok(j) => j,
                                Err(_) => continue,
                            };
                            let _ = event_tx.send(json);
                        }
                        None => break,
                    }
                }
                input = input_rx.recv() => {
                    match input {
                        Some(SessionInput::Prompt(text)) => {
                            if let Err(e) = process.send_prompt(&text).await {
                                tracing::warn!("ACP send_prompt failed for {}: {}", sid, e);
                            }
                        }
                        Some(SessionInput::Cancel) => {
                            process.kill().await;
                        }
                        None => break, // all input senders dropped (session removed)
                        _ => {} // ignore PTY commands
                    }
                }
            }
        }
        tracing::info!("ACP fan-out task ended for session {}", sid);
    });
}

fn spawn_kiro_fanout(
    sid: String,
    mut process: KiroProcess,
    event_tx: broadcast::Sender<String>,
    mut input_rx: mpsc::Receiver<SessionInput>,
) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                event = process.event_rx.recv() => {
                    match event {
                        Some(evt) => {
                            let json = match serde_json::to_string(&evt) {
                                Ok(j) => j,
                                Err(_) => continue,
                            };
                            let _ = event_tx.send(json);
                        }
                        None => break,
                    }
                }
                input = input_rx.recv() => {
                    match input {
                        Some(SessionInput::Prompt(text)) => {
                            if let Err(e) = process.send_prompt(&text).await {
                                tracing::warn!("Kiro send_prompt failed for {}: {}", sid, e);
                            }
                        }
                        Some(SessionInput::Cancel) => {
                            process.kill().await;
                        }
                        None => break,
                        _ => {}
                    }
                }
            }
        }
        tracing::info!("Kiro fan-out task ended for session {}", sid);
    });
}

fn spawn_codex_fanout(
    sid: String,
    mut process: crate::acp::codex_process::CodexProcess,
    event_tx: broadcast::Sender<String>,
    mut input_rx: mpsc::Receiver<SessionInput>,
) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                event = process.event_rx.recv() => {
                    match event {
                        Some(evt) => {
                            let json = match serde_json::to_string(&evt) {
                                Ok(j) => j,
                                Err(_) => continue,
                            };
                            let _ = event_tx.send(json);
                        }
                        None => break,
                    }
                }
                input = input_rx.recv() => {
                    match input {
                        Some(SessionInput::Prompt(text)) => {
                            if let Err(e) = process.send_prompt(&text) {
                                tracing::warn!("Codex send_prompt failed for {}: {}", sid, e);
                            }
                        }
                        Some(SessionInput::Cancel) => {
                            process.kill().await;
                        }
                        None => break,
                        _ => {}
                    }
                }
            }
        }
        tracing::info!("Codex fan-out task ended for session {}", sid);
    });
}
