use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Live activity reported by agent-CLI hooks running inside tmux sessions.
/// Keyed by tmux session name (stable across zeromux restarts), not by the
/// ephemeral zeromux session UUID.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SessionActivity {
    /// "running" | "idle" | "needs_input"
    pub state: String,
    /// "finished" | "needs_input" — set on running→idle and on needs_input,
    /// cleared when the user focuses the session or a new prompt starts.
    pub attention: Option<String>,
    pub updated_at: String,
}

pub struct ActivityStore {
    activity: Mutex<HashMap<String, SessionActivity>>,
    titles: Mutex<HashMap<String, String>>,
    titles_path: PathBuf,
    generating: Mutex<HashSet<String>>,
}

/// Derive the per-tmux-session hook token from the JWT secret. Stateless, so
/// it survives server restarts as long as ZEROMUX_JWT_SECRET is stable.
pub fn activity_token(secret: &str, tmux_name: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"zeromux-activity:");
    hasher.update(secret.as_bytes());
    hasher.update(b":");
    hasher.update(tmux_name.as_bytes());
    hex::encode(hasher.finalize())
}

impl ActivityStore {
    pub fn open(data_dir: &Path) -> Self {
        let titles_path = data_dir.join("session_titles.json");
        let titles: HashMap<String, String> = std::fs::read_to_string(&titles_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            activity: Mutex::new(HashMap::new()),
            titles: Mutex::new(titles),
            titles_path,
            generating: Mutex::new(HashSet::new()),
        }
    }

    /// Record a hook report. Returns true if this report should trigger
    /// auto-title generation (a prompt arrived and no title exists yet).
    pub fn report(&self, tmux_name: &str, state: &str, has_prompt: bool) -> bool {
        let mut map = self.activity.lock().unwrap();
        let prev = map.get(tmux_name).map(|a| a.state.clone());
        let attention = match state {
            "idle" if prev.as_deref() == Some("running") => Some("finished".to_string()),
            "idle" => map.get(tmux_name).and_then(|a| a.attention.clone()),
            "needs_input" => Some("needs_input".to_string()),
            _ => None, // "running" clears attention
        };
        map.insert(
            tmux_name.to_string(),
            SessionActivity {
                state: state.to_string(),
                attention,
                updated_at: crate::events::now_iso(),
            },
        );
        drop(map);

        has_prompt && !self.titles.lock().unwrap().contains_key(tmux_name)
    }

    pub fn get(&self, tmux_name: &str) -> Option<SessionActivity> {
        self.activity.lock().unwrap().get(tmux_name).cloned()
    }

    pub fn clear_attention(&self, tmux_name: &str) {
        if let Some(a) = self.activity.lock().unwrap().get_mut(tmux_name) {
            a.attention = None;
        }
    }

    pub fn title(&self, tmux_name: &str) -> Option<String> {
        self.titles.lock().unwrap().get(tmux_name).cloned()
    }

    pub fn set_title(&self, tmux_name: &str, title: String) {
        let mut titles = self.titles.lock().unwrap();
        titles.insert(tmux_name.to_string(), title);
        self.save_titles(&titles);
    }

    /// Drop all state for a tmux session (called when it is killed, so a
    /// reused session name doesn't inherit a stale title).
    pub fn forget(&self, tmux_name: &str) {
        self.activity.lock().unwrap().remove(tmux_name);
        let mut titles = self.titles.lock().unwrap();
        if titles.remove(tmux_name).is_some() {
            self.save_titles(&titles);
        }
    }

    fn save_titles(&self, titles: &HashMap<String, String>) {
        let tmp = self.titles_path.with_extension("json.tmp");
        if let Ok(json) = serde_json::to_string_pretty(titles) {
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &self.titles_path);
            }
        }
    }

    fn begin_generation(&self, tmux_name: &str) -> bool {
        self.generating.lock().unwrap().insert(tmux_name.to_string())
    }

    fn end_generation(&self, tmux_name: &str) {
        self.generating.lock().unwrap().remove(tmux_name);
    }
}

/// Generate a short task-style title from the session's first prompt using a
/// cheap model via the AWS CLI (`bedrock-runtime converse`), then store it.
/// Fire-and-forget; failures just leave the session untitled.
pub fn spawn_title_generation(
    state: std::sync::Arc<crate::AppState>,
    tmux_name: String,
    prompt: String,
) {
    if !state.activity.begin_generation(&tmux_name) {
        return; // already in flight
    }
    tokio::task::spawn_blocking(move || {
        let title = generate_title(&prompt);
        match title {
            Some(t) => {
                tracing::info!("Auto-title for {}: {}", tmux_name, t);
                state.activity.set_title(&tmux_name, t);
            }
            None => tracing::warn!("Auto-title generation failed for {}", tmux_name),
        }
        state.activity.end_generation(&tmux_name);
    });
}

fn generate_title(prompt: &str) -> Option<String> {
    let model = std::env::var("ZEROMUX_TITLE_MODEL")
        .unwrap_or_else(|_| "global.anthropic.claude-haiku-4-5-20251001-v1:0".to_string());
    let excerpt: String = prompt.chars().take(500).collect();
    let messages = serde_json::json!([{
        "role": "user",
        "content": [{"text": format!(
            "Generate a concise title (3-6 words, same language as the prompt, no quotes, no trailing punctuation) describing the TASK in this coding-agent prompt:\n\n{}",
            excerpt
        )}]
    }]);

    let output = std::process::Command::new("aws")
        .args([
            "bedrock-runtime",
            "converse",
            "--model-id",
            &model,
            "--messages",
            &messages.to_string(),
            "--inference-config",
            "{\"maxTokens\":40}",
            "--query",
            "output.message.content[0].text",
            "--output",
            "text",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        tracing::warn!(
            "bedrock converse failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }
    let title = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if title.is_empty() || title.len() > 120 {
        return None;
    }
    Some(title)
}
