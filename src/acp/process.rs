use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::mpsc;

// ── Events we send to the browser ──

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum AcpEvent {
    /// System init / status
    #[serde(rename = "system")]
    System {
        subtype: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },

    /// One content block from an assistant message
    /// Could be text, thinking, or tool_use
    #[serde(rename = "content_block")]
    ContentBlock {
        block_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<serde_json::Value>,
        /// When true, this is a streaming delta — frontend should append to last text block
        #[serde(skip_serializing_if = "Option::is_none")]
        streaming: Option<bool>,
    },

    /// Turn completed
    #[serde(rename = "result")]
    Result {
        text: String,
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cost_usd: Option<f64>,
    },

    /// Error
    #[serde(rename = "error")]
    Error { message: String },

    /// Process exited
    #[serde(rename = "exit")]
    Exit { code: i32 },
}

// ── Raw NDJSON from Claude CLI stdout ──

#[derive(Debug, Deserialize)]
struct RawEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    /// assistant message: { role: "assistant", content: [...] }
    #[serde(default)]
    message: Option<AssistantMessage>,
    /// result final text
    #[serde(default)]
    result: Option<String>,
    /// cost
    #[serde(default)]
    total_cost_usd: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct AssistantMessage {
    #[serde(default)]
    content: Vec<ContentBlock>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    /// for "text" and "thinking" blocks
    #[serde(default)]
    text: Option<String>,
    /// for "tool_use" blocks
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<serde_json::Value>,
}

pub struct AcpProcess {
    child: Child,
    stdin: ChildStdin,
    pub event_rx: mpsc::Receiver<AcpEvent>,
}

impl AcpProcess {
    pub async fn spawn(
        claude_path: &str,
        work_dir: &str,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let args = [
            "-p",
            "--output-format", "stream-json",
            "--input-format", "stream-json",
            "--verbose",
            "--dangerously-skip-permissions",
        ];

        let mut child = tokio::process::Command::new(claude_path)
            .args(&args)
            .current_dir(work_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        let (tx, rx) = mpsc::channel::<AcpEvent>(256);

        tokio::spawn(Self::read_events(stdout, tx));

        Ok(Self {
            child,
            stdin,
            event_rx: rx,
        })
    }

    async fn read_events(stdout: ChildStdout, tx: mpsc::Sender<AcpEvent>) {
        let reader = BufReader::with_capacity(1024 * 1024, stdout); // 1MB buffer like naozhi
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }

            let raw: RawEvent = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!("stream-json parse error: {} — {}", e, &line[..line.len().min(200)]);
                    continue;
                }
            };

            let events = Self::translate(&raw);
            for evt in events {
                if tx.send(evt).await.is_err() {
                    return;
                }
            }
        }

        let _ = tx.send(AcpEvent::Exit { code: 0 }).await;
    }

    /// Translate one raw Claude NDJSON line into zero or more browser events
    fn translate(raw: &RawEvent) -> Vec<AcpEvent> {
        match raw.event_type.as_str() {
            "system" => {
                let subtype = raw.subtype.clone().unwrap_or_default();
                // Skip hook events (noisy, not useful for browser)
                if subtype == "hook_started" || subtype == "hook_response" {
                    return vec![];
                }
                vec![AcpEvent::System {
                    subtype,
                    session_id: raw.session_id.clone(),
                }]
            }

            "assistant" => {
                // Each assistant event has message.content[] with content blocks.
                // We emit one ContentBlock event per block so the browser can
                // render text, thinking, and tool_use separately.
                let Some(msg) = &raw.message else {
                    return vec![];
                };
                msg.content
                    .iter()
                    .map(|block| AcpEvent::ContentBlock {
                        block_type: block.block_type.clone(),
                        text: block.text.clone(),
                        name: block.name.clone(),
                        input: block.input.clone(),
                        streaming: None,
                    })
                    .collect()
            }

            "result" => {
                vec![AcpEvent::Result {
                    text: raw.result.clone().unwrap_or_default(),
                    session_id: raw.session_id.clone().unwrap_or_default(),
                    cost_usd: raw.total_cost_usd,
                }]
            }

            _ => {
                tracing::debug!("Unknown stream-json event: {}", raw.event_type);
                vec![]
            }
        }
    }

    /// Send a user message via stdin (stream-json NDJSON format)
    pub async fn send_prompt(&mut self, text: &str) -> Result<(), std::io::Error> {
        let msg = serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": text}]
            }
        });
        let mut line = serde_json::to_string(&msg).unwrap();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
    }
}

impl Drop for AcpProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}
