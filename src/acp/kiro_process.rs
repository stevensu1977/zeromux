use serde::Deserialize;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::mpsc;

use super::process::AcpEvent;

// ── JSON-RPC 2.0 message ──

#[derive(Debug, Deserialize)]
struct RpcMessage {
    #[serde(default)]
    id: Option<serde_json::Value>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<RpcError>,
    #[serde(default)]
    params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

impl RpcMessage {
    fn is_response(&self) -> bool {
        self.id.is_some() && self.method.is_none()
    }
    fn is_request(&self) -> bool {
        self.id.is_some() && self.method.is_some()
    }
    fn is_notification(&self) -> bool {
        self.id.is_none() && self.method.is_some()
    }
}

// ── Session update structs ──

#[derive(Debug, Deserialize)]
struct SessionUpdateParams {
    #[allow(dead_code)]
    #[serde(rename = "sessionId")]
    session_id: String,
    update: UpdatePayload,
}

#[derive(Debug, Deserialize)]
struct UpdatePayload {
    #[serde(rename = "sessionUpdate")]
    session_update: String,
    #[serde(default)]
    content: Option<serde_json::Value>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TextContent {
    #[serde(default)]
    text: Option<String>,
}

// ── Internal command channel ──

enum PromptCmd {
    Send { text: String },
    Kill,
}

// ── KiroProcess ──

pub struct KiroProcess {
    child: Child,
    prompt_tx: mpsc::Sender<PromptCmd>,
    pub event_rx: mpsc::Receiver<AcpEvent>,
}

impl KiroProcess {
    pub async fn spawn(
        kiro_path: &str,
        work_dir: &str,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let mut child = tokio::process::Command::new(kiro_path)
            .args(["acp", "--trust-all-tools"])
            .current_dir(work_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let reader = BufReader::with_capacity(1024 * 1024, stdout);
        let mut lines = reader.lines();

        // ── Initialize handshake ──
        Self::send_rpc(&mut stdin, 0, "initialize", serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": { "readTextFile": true, "writeTextFile": true },
                "terminal": true
            },
            "clientInfo": { "name": "zeromux", "version": "1.0.0" }
        })).await?;

        Self::wait_response(&mut lines).await?;

        // ── session/new ──
        let cwd = if work_dir == "." {
            std::env::current_dir()?.to_string_lossy().to_string()
        } else {
            work_dir.to_string()
        };

        Self::send_rpc(&mut stdin, 1, "session/new", serde_json::json!({
            "cwd": cwd,
            "mcpServers": []
        })).await?;

        let resp = Self::wait_response(&mut lines).await?;
        let session_id = resp
            .and_then(|r| r.get("sessionId").and_then(|v| v.as_str().map(String::from)))
            .unwrap_or_else(|| "unknown".to_string());

        // ── Set up event loop ──
        let (event_tx, event_rx) = mpsc::channel::<AcpEvent>(256);
        let (prompt_tx, prompt_rx) = mpsc::channel::<PromptCmd>(16);

        let _ = event_tx.send(AcpEvent::System {
            subtype: "init".to_string(),
            session_id: Some(session_id.clone()),
        }).await;

        tokio::spawn(Self::event_loop(lines, stdin, event_tx, prompt_rx, session_id));

        Ok(Self { child, prompt_tx, event_rx })
    }

    async fn send_rpc(
        stdin: &mut ChildStdin,
        id: i64,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), std::io::Error> {
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut line = serde_json::to_string(&req).unwrap();
        line.push('\n');
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await
    }

    /// Wait for a JSON-RPC response, skipping notifications
    async fn wait_response(
        lines: &mut tokio::io::Lines<BufReader<ChildStdout>>,
    ) -> Result<Option<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>> {
        loop {
            let Some(raw) = lines.next_line().await? else {
                return Err("kiro-cli closed during handshake".into());
            };
            if raw.trim().is_empty() { continue; }
            let msg: RpcMessage = serde_json::from_str(&raw)?;
            if msg.is_response() {
                if let Some(err) = msg.error {
                    return Err(format!("RPC error {}: {}", err.code, err.message).into());
                }
                return Ok(msg.result);
            }
            // skip notifications during handshake
        }
    }

    async fn event_loop(
        mut lines: tokio::io::Lines<BufReader<ChildStdout>>,
        mut stdin: ChildStdin,
        tx: mpsc::Sender<AcpEvent>,
        mut prompt_rx: mpsc::Receiver<PromptCmd>,
        session_id: String,
    ) {
        let mut next_id: i64 = 2;
        let mut text_buf = String::new();

        loop {
            tokio::select! {
                line_result = lines.next_line() => {
                    match line_result {
                        Ok(Some(line)) if !line.trim().is_empty() => {
                            let msg: RpcMessage = match serde_json::from_str(&line) {
                                Ok(m) => m,
                                Err(e) => {
                                    tracing::debug!("kiro parse: {} — {}", e, &line[..line.len().min(200)]);
                                    continue;
                                }
                            };
                            let events = Self::handle_msg(&msg, &mut text_buf, &session_id, &mut stdin).await;
                            for evt in events {
                                if tx.send(evt).await.is_err() { return; }
                            }
                        }
                        Ok(Some(_)) => continue, // empty line
                        _ => {
                            let _ = tx.send(AcpEvent::Exit { code: 0 }).await;
                            return;
                        }
                    }
                }

                cmd = prompt_rx.recv() => {
                    match cmd {
                        Some(PromptCmd::Send { text }) => {
                            text_buf.clear();
                            let req = serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": next_id,
                                "method": "session/prompt",
                                "params": {
                                    "sessionId": session_id,
                                    "prompt": [{ "type": "text", "text": text }]
                                }
                            });
                            next_id += 1;
                            let mut line = serde_json::to_string(&req).unwrap();
                            line.push('\n');
                            if stdin.write_all(line.as_bytes()).await.is_err() { return; }
                            let _ = stdin.flush().await;
                        }
                        Some(PromptCmd::Kill) | None => return,
                    }
                }
            }
        }
    }

    async fn handle_msg(
        msg: &RpcMessage,
        text_buf: &mut String,
        session_id: &str,
        stdin: &mut ChildStdin,
    ) -> Vec<AcpEvent> {
        // Response → turn complete
        if msg.is_response() {
            if let Some(err) = &msg.error {
                return vec![AcpEvent::Error {
                    message: format!("RPC error {}: {}", err.code, err.message),
                }];
            }
            let result_text = text_buf.clone();
            text_buf.clear();
            return vec![AcpEvent::Result {
                text: result_text,
                session_id: session_id.to_string(),
                cost_usd: None,
            }];
        }

        // Server request → auto-approve permissions
        if msg.is_request() {
            if msg.method.as_deref() == Some("session/request_permission") {
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": msg.id,
                    "result": {
                        "outcome": { "outcome": "selected", "optionId": "allow-once" }
                    }
                });
                let mut line = serde_json::to_string(&resp).unwrap();
                line.push('\n');
                let _ = stdin.write_all(line.as_bytes()).await;
                let _ = stdin.flush().await;
            }
            return vec![];
        }

        // Notification → session/update
        if msg.is_notification() && msg.method.as_deref() == Some("session/update") {
            if let Some(params) = &msg.params {
                if let Ok(up) = serde_json::from_value::<SessionUpdateParams>(params.clone()) {
                    return Self::handle_update(&up, text_buf);
                }
            }
        }

        vec![]
    }

    fn handle_update(update: &SessionUpdateParams, text_buf: &mut String) -> Vec<AcpEvent> {
        match update.update.session_update.as_str() {
            "agent_message_chunk" => {
                if let Some(content) = &update.update.content {
                    if let Ok(tc) = serde_json::from_value::<TextContent>(content.clone()) {
                        if let Some(text) = &tc.text {
                            text_buf.push_str(text);
                            return vec![AcpEvent::ContentBlock {
                                block_type: "text".to_string(),
                                text: Some(text.clone()),
                                name: None,
                                input: None,
                                streaming: Some(true),
                            }];
                        }
                    }
                }
                vec![]
            }
            "tool_call" => {
                vec![AcpEvent::ContentBlock {
                    block_type: "tool_use".to_string(),
                    text: None,
                    name: Some(update.update.title.clone().unwrap_or_else(|| "tool".to_string())),
                    input: None,
                    streaming: None,
                }]
            }
            "tool_call_update" => vec![],
            _ => {
                tracing::debug!("Unknown kiro update: {}", update.update.session_update);
                vec![]
            }
        }
    }

    pub async fn send_prompt(&mut self, text: &str) -> Result<(), std::io::Error> {
        self.prompt_tx
            .send(PromptCmd::Send { text: text.to_string() })
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "kiro process gone"))
    }

    pub async fn kill(&mut self) {
        let _ = self.prompt_tx.send(PromptCmd::Kill).await;
        let _ = self.child.kill().await;
    }
}

impl Drop for KiroProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}
