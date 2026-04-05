use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tokio::sync::mpsc;

/// Max scrollback buffer size in bytes (2MB of encoded data)
const SCROLLBACK_MAX_BYTES: usize = 2 * 1024 * 1024;

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
}

impl std::fmt::Display for SessionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionType::Tmux => write!(f, "tmux"),
            SessionType::Claude => write!(f, "claude"),
            SessionType::Kiro => write!(f, "kiro"),
        }
    }
}

/// A terminal session backed by PTY (for tmux/bash)
struct PtySession {
    pty: PtyHandle,
    output_rx: Option<mpsc::Receiver<Vec<u8>>>,
}

/// A Claude Code session backed by stream-json protocol
struct AcpSession {
    process: Option<AcpProcess>,
}

/// A Kiro session backed by JSON-RPC 2.0 ACP protocol
struct KiroSession {
    process: Option<KiroProcess>,
}

enum SessionBackend {
    Pty(PtySession),
    Acp(AcpSession),
    Kiro(KiroSession),
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
    pub notes: String,
    backend: SessionBackend,
    /// Git worktree path for ACP sessions (cleaned up on delete)
    worktree_path: Option<PathBuf>,
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
    pub notes: String,
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
            // Fallback: try to remove directory directly
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

impl SessionManager {
    pub fn new() -> Self {
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
    ) -> Result<String, String> {
        let cwd = if work_dir.is_empty() || work_dir == "." {
            None
        } else {
            Some(work_dir)
        };
        let (pty, rx) = PtyHandle::spawn(shell, &[], &[], cols, rows, cwd)
            .map_err(|e| format!("Failed to spawn PTY: {}", e))?;

        let effective_dir = if work_dir.is_empty() || work_dir == "." {
            std::env::current_dir().unwrap_or_default().to_string_lossy().to_string()
        } else {
            work_dir.to_string()
        };

        let id = uuid::Uuid::new_v4().to_string();
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
            notes: String::new(),
            backend: SessionBackend::Pty(PtySession {
                pty,
                output_rx: Some(rx),
            }),
            worktree_path: None,
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
                // Clean up worktree if spawn fails
                if let Some(wt) = &worktree_path {
                    let base = PathBuf::from(work_dir);
                    remove_worktree(&base, wt);
                }
                format!("Failed to spawn Claude: {}", e)
            })?;

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
            notes: String::new(),
            backend: SessionBackend::Acp(AcpSession {
                process: Some(process),
            }),
            worktree_path,
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
            notes: String::new(),
            backend: SessionBackend::Kiro(KiroSession {
                process: Some(process),
            }),
            worktree_path,
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
            .filter(|s| {
                owner_filter
                    .map(|uid| s.owner_id == uid)
                    .unwrap_or(true)
            })
            .map(|s| SessionInfo {
                id: s.id.clone(),
                name: s.name.clone(),
                session_type: s.session_type,
                cols: s.cols,
                rows: s.rows,
                work_dir: s.work_dir.clone(),
                description: s.description.clone(),
                status: s.status,
                notes: s.notes.clone(),
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

    pub fn remove_session(&self, id: &str) -> bool {
        let removed = self.sessions.lock().unwrap().remove(id);
        if let Some(session) = removed {
            // Clean up git worktree if one was created
            if let Some(wt_path) = &session.worktree_path {
                // Find the repo root (parent of .zeromux-worktrees)
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

    // --- PTY session methods ---

    pub fn take_output_rx(&self, id: &str) -> Option<mpsc::Receiver<Vec<u8>>> {
        self.sessions
            .lock()
            .unwrap()
            .get_mut(id)
            .and_then(|s| match &mut s.backend {
                SessionBackend::Pty(pty) => pty.output_rx.take(),
                _ => None,
            })
    }

    pub fn return_output_rx(&self, id: &str, rx: mpsc::Receiver<Vec<u8>>) {
        if let Some(session) = self.sessions.lock().unwrap().get_mut(id) {
            if let SessionBackend::Pty(pty) = &mut session.backend {
                pty.output_rx = Some(rx);
            }
        }
    }

    pub fn write_to_session(&self, id: &str, data: &[u8]) -> Result<(), String> {
        self.sessions
            .lock()
            .unwrap()
            .get_mut(id)
            .ok_or_else(|| "Session not found".to_string())
            .and_then(|s| match &mut s.backend {
                SessionBackend::Pty(pty) => pty.pty.write_input(data).map_err(|e| e.to_string()),
                _ => Err("Not a PTY session".to_string()),
            })
    }

    pub fn resize_session(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(id)
            .ok_or_else(|| "Session not found".to_string())?;
        session.cols = cols;
        session.rows = rows;
        match &session.backend {
            SessionBackend::Pty(pty) => pty.pty.resize(cols, rows).map_err(|e| e.to_string()),
            SessionBackend::Acp(_) | SessionBackend::Kiro(_) => Ok(()),
        }
    }

    // --- ACP session methods ---

    pub fn take_acp_process(&self, id: &str) -> Option<AcpProcess> {
        self.sessions
            .lock()
            .unwrap()
            .get_mut(id)
            .and_then(|s| match &mut s.backend {
                SessionBackend::Acp(acp) => acp.process.take(),
                _ => None,
            })
    }

    pub fn return_acp_process(&self, id: &str, process: AcpProcess) {
        if let Some(session) = self.sessions.lock().unwrap().get_mut(id) {
            if let SessionBackend::Acp(acp) = &mut session.backend {
                acp.process = Some(process);
            }
        }
    }

    // --- Kiro session methods ---

    pub fn take_kiro_process(&self, id: &str) -> Option<KiroProcess> {
        self.sessions
            .lock()
            .unwrap()
            .get_mut(id)
            .and_then(|s| match &mut s.backend {
                SessionBackend::Kiro(k) => k.process.take(),
                _ => None,
            })
    }

    pub fn return_kiro_process(&self, id: &str, process: KiroProcess) {
        if let Some(session) = self.sessions.lock().unwrap().get_mut(id) {
            if let SessionBackend::Kiro(k) = &mut session.backend {
                k.process = Some(process);
            }
        }
    }

    /// Update session metadata (description, status, notes)
    pub fn update_session_meta(
        &self,
        id: &str,
        description: Option<String>,
        status: Option<SessionMeta>,
        notes: Option<String>,
    ) -> bool {
        if let Some(session) = self.sessions.lock().unwrap().get_mut(id) {
            if let Some(d) = description {
                session.description = d;
            }
            if let Some(s) = status {
                session.status = s;
            }
            if let Some(n) = notes {
                session.notes = n;
            }
            true
        } else {
            false
        }
    }

    /// Get session type for a given id
    pub fn session_type(&self, id: &str) -> Option<SessionType> {
        self.sessions.lock().unwrap().get(id).map(|s| s.session_type)
    }

    /// Push output data to the scrollback buffer (base64 for PTY, JSON for ACP/Kiro)
    pub fn push_scrollback(&self, id: &str, data: String) {
        if let Some(s) = self.sessions.lock().unwrap().get_mut(id) {
            let data_len = data.len();
            s.scrollback.push_back(data);
            s.scrollback_bytes += data_len;
            // Trim from front if over limit
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
        self.sessions.lock().unwrap().get(id).map(|s| s.work_dir.clone())
    }

    /// Get PTY child PID for a session
    pub fn pty_pid(&self, id: &str) -> Option<u32> {
        self.sessions.lock().unwrap().get(id).and_then(|s| match &s.backend {
            SessionBackend::Pty(pty) => pty.pty.pid(),
            _ => None,
        })
    }
}
