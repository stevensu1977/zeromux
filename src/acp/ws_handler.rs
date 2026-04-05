use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, Query, State, WebSocketUpgrade,
    },
    response::Response,
};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;

use super::process::AcpEvent;
use crate::session_manager::SessionType;
use crate::{auth, AppState};

#[derive(serde::Deserialize)]
pub struct WsQuery {
    pub token: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(tag = "type")]
enum ClientMsg {
    #[serde(rename = "prompt")]
    Prompt { text: String },
    #[serde(rename = "cancel")]
    Cancel,
}

/// Trait to unify Claude AcpProcess and KiroProcess for the WS handler
trait AgentProcess: Send {
    fn event_rx(&mut self) -> &mut tokio::sync::mpsc::Receiver<AcpEvent>;
    fn send_prompt(&mut self, text: &str) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send;
    fn kill(&mut self) -> impl std::future::Future<Output = ()> + Send;
}

impl AgentProcess for super::process::AcpProcess {
    fn event_rx(&mut self) -> &mut tokio::sync::mpsc::Receiver<AcpEvent> {
        &mut self.event_rx
    }
    async fn send_prompt(&mut self, text: &str) -> Result<(), std::io::Error> {
        self.send_prompt(text).await
    }
    async fn kill(&mut self) {
        self.kill().await
    }
}

impl AgentProcess for super::kiro_process::KiroProcess {
    fn event_rx(&mut self) -> &mut tokio::sync::mpsc::Receiver<AcpEvent> {
        &mut self.event_rx
    }
    async fn send_prompt(&mut self, text: &str) -> Result<(), std::io::Error> {
        self.send_prompt(text).await
    }
    async fn kill(&mut self) {
        self.kill().await
    }
}

pub async fn ws_acp(
    ws: WebSocketUpgrade,
    Path(session_id): Path<String>,
    Query(query): Query<WsQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let authed = query
        .token
        .as_ref()
        .and_then(|t| auth::verify_ws_token(&state, t))
        .is_some();

    if !authed {
        return Response::builder()
            .status(401)
            .body(axum::body::Body::from("Unauthorized"))
            .unwrap();
    }

    ws.on_upgrade(move |socket| handle_acp_ws(socket, session_id, state))
}

async fn handle_acp_ws(socket: WebSocket, session_id: String, state: Arc<AppState>) {
    let session_type = state.sessions.session_type(&session_id);

    match session_type {
        Some(SessionType::Claude) => {
            if let Some(process) = state.sessions.take_acp_process(&session_id) {
                run_agent_ws(socket, &session_id, process, &state, |st, id, p| {
                    st.sessions.return_acp_process(id, p);
                }).await;
            } else {
                tracing::error!("ACP session {} not found or already connected", session_id);
            }
        }
        Some(SessionType::Kiro) => {
            if let Some(process) = state.sessions.take_kiro_process(&session_id) {
                run_agent_ws(socket, &session_id, process, &state, |st, id, p| {
                    st.sessions.return_kiro_process(id, p);
                }).await;
            } else {
                tracing::error!("Kiro session {} not found or already connected", session_id);
            }
        }
        _ => {
            tracing::error!("Session {} is not an ACP session", session_id);
        }
    }
}

async fn run_agent_ws<P, F>(
    socket: WebSocket,
    session_id: &str,
    mut process: P,
    state: &Arc<AppState>,
    return_fn: F,
) where
    P: AgentProcess + 'static,
    F: FnOnce(&AppState, &str, P),
{
    let (mut ws_sink, mut ws_stream) = socket.split();

    let init_msg = serde_json::json!({"type": "system", "message": "connected"});
    let _ = ws_sink
        .send(Message::Text(init_msg.to_string().into()))
        .await;

    // Replay event history for reconnecting clients
    let history = state.sessions.get_scrollback(session_id);
    let has_history = !history.is_empty();
    for json in history {
        if ws_sink
            .send(Message::Text(json.into()))
            .await
            .is_err()
        {
            return_fn(state, session_id, process);
            return;
        }
    }
    // Signal that replay is done so the frontend can reset busy state
    if has_history {
        let done_msg = serde_json::json!({"type": "replay_done"});
        let _ = ws_sink.send(Message::Text(done_msg.to_string().into())).await;
    }

    let sid = session_id.to_string();
    let logger = state.logger.clone();

    // Single select! loop — no spawned tasks, so process is always returned promptly
    loop {
        tokio::select! {
            event = process.event_rx().recv() => {
                match event {
                    Some(evt) => {
                        let is_exit = matches!(evt, AcpEvent::Exit { .. });
                        let json = match serde_json::to_string(&evt) {
                            Ok(j) => j,
                            Err(_) => continue,
                        };

                        // Log ACP event
                        if let Some(ref log) = logger {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&json) {
                                log.log_acp_event(&sid, &val);
                            }
                        }

                        // Push to scrollback buffer for future reconnects
                        state.sessions.push_scrollback(&sid, json.clone());

                        if ws_sink.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                        if is_exit {
                            break;
                        }
                    }
                    None => break,
                }
            }

            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(client_msg) = serde_json::from_str::<ClientMsg>(&text) {
                            match client_msg {
                                ClientMsg::Prompt { text } => {
                                    if let Some(ref log) = logger {
                                        log.log_acp_input(&sid, &text);
                                    }
                                    if let Err(e) = process.send_prompt(&text).await {
                                        let err = serde_json::json!({"type": "error", "message": format!("Send failed: {}", e)});
                                        let _ = ws_sink.send(Message::Text(err.to_string().into())).await;
                                    }
                                }
                                ClientMsg::Cancel => {
                                    process.kill().await;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }

    return_fn(state, &sid, process);
    tracing::info!("ACP WebSocket disconnected for session {}", sid);
}
