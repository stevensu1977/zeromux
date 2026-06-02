use std::process::{Command, Stdio};
use tokio::sync::mpsc;

use super::process::AcpEvent;

pub struct CodexProcess {
    pub event_rx: mpsc::Receiver<AcpEvent>,
    cmd_tx: mpsc::Sender<String>,
}

impl CodexProcess {
    pub async fn spawn(
        codex_path: &str,
        work_dir: &str,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let (event_tx, event_rx) = mpsc::channel::<AcpEvent>(256);
        let (cmd_tx, cmd_rx) = mpsc::channel::<String>(16);

        let codex_path = codex_path.to_string();
        let work_dir = work_dir.to_string();

        // Send initial system event
        let _ = event_tx.send(AcpEvent::System {
            subtype: "session".to_string(),
            session_id: Some("codex".to_string()),
        }).await;

        // Background task: receives prompts, spawns codex exec for each
        tokio::spawn(async move {
            run_codex_loop(codex_path, work_dir, cmd_rx, event_tx).await;
        });

        Ok(Self { event_rx, cmd_tx })
    }

    pub fn send_prompt(&mut self, text: &str) -> Result<(), std::io::Error> {
        self.cmd_tx.try_send(text.to_string())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e.to_string()))
    }

    pub async fn kill(&mut self) {
        // Dropping cmd_tx will cause the loop to exit
        drop(self.cmd_tx.clone());
    }
}

async fn run_codex_loop(
    codex_path: String,
    work_dir: String,
    mut cmd_rx: mpsc::Receiver<String>,
    event_tx: mpsc::Sender<AcpEvent>,
) {
    while let Some(prompt) = cmd_rx.recv().await {
        // Spawn codex exec for this prompt
        let result = tokio::task::spawn_blocking({
            let codex_path = codex_path.clone();
            let work_dir = work_dir.clone();
            let event_tx = event_tx.clone();
            let prompt = prompt.clone();
            move || {
                run_single_codex(&codex_path, &work_dir, &prompt, &event_tx)
            }
        }).await;

        if let Err(e) = result {
            let _ = event_tx.send(AcpEvent::Error {
                message: format!("Codex execution failed: {}", e),
            }).await;
        }
    }

    let _ = event_tx.send(AcpEvent::Exit { code: 0 }).await;
}

fn run_single_codex(
    codex_path: &str,
    work_dir: &str,
    prompt: &str,
    event_tx: &mpsc::Sender<AcpEvent>,
) {
    use std::io::{BufRead, BufReader, Write};

    let mut child = match Command::new(codex_path)
        .args([
            "exec",
            "--json",
            "--dangerously-bypass-approvals-and-sandbox",
            "--ephemeral",
            "-C",
            work_dir,
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = event_tx.blocking_send(AcpEvent::Error {
                message: format!("Failed to spawn codex: {}", e),
            });
            return;
        }
    };

    // Write prompt and close stdin
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
        let _ = stdin.write_all(b"\n");
        // stdin drops here, closing it
    }

    // Read JSONL output
    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::with_capacity(256 * 1024, stdout);
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            if line.is_empty() {
                continue;
            }

            let raw: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let events = translate_codex_event(&raw);
            for event in events {
                if event_tx.blocking_send(event).is_err() {
                    let _ = child.kill();
                    return;
                }
            }
        }
    }

    let _ = child.wait();
}

fn translate_codex_event(raw: &serde_json::Value) -> Vec<AcpEvent> {
    let event_type = raw["type"].as_str().unwrap_or("");

    match event_type {
        "thread.started" => vec![],

        "turn.started" => vec![],

        "item.started" => {
            let item = &raw["item"];
            let item_type = item["type"].as_str().unwrap_or("");

            if item_type == "command_execution" {
                let command = item["command"].as_str().unwrap_or("").to_string();
                vec![AcpEvent::ContentBlock {
                    block_type: "tool_use".to_string(),
                    text: None,
                    name: Some("shell".to_string()),
                    input: Some(serde_json::json!({ "command": command })),
                    streaming: Some(false),
                }]
            } else {
                vec![]
            }
        }

        "item.completed" => {
            let item = &raw["item"];
            let item_type = item["type"].as_str().unwrap_or("");

            match item_type {
                "command_execution" => {
                    let output = item["aggregated_output"].as_str().unwrap_or("").to_string();
                    let exit_code = item["exit_code"].as_i64().unwrap_or(-1);
                    let command = item["command"].as_str().unwrap_or("").to_string();
                    vec![AcpEvent::ContentBlock {
                        block_type: "tool_result".to_string(),
                        text: Some(format!("$ {}\n{}\n[exit: {}]", command, output.trim(), exit_code)),
                        name: Some("shell".to_string()),
                        input: None,
                        streaming: Some(false),
                    }]
                }
                "agent_message" => {
                    let text = item["text"].as_str().unwrap_or("").to_string();
                    vec![AcpEvent::ContentBlock {
                        block_type: "text".to_string(),
                        text: Some(text),
                        name: None,
                        input: None,
                        streaming: Some(false),
                    }]
                }
                _ => vec![],
            }
        }

        "turn.completed" => {
            vec![AcpEvent::Result {
                text: String::new(),
                session_id: String::new(),
                cost_usd: None,
            }]
        }

        _ => vec![],
    }
}
