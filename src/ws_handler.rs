use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, Query, State, WebSocketUpgrade,
    },
    response::Response,
};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;

use crate::{auth, AppState};

#[derive(serde::Deserialize)]
pub struct WsQuery {
    pub token: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(tag = "type")]
enum ClientMsg {
    #[serde(rename = "input")]
    Input { data: String },
    #[serde(rename = "resize")]
    Resize { cols: u16, rows: u16 },
}

pub async fn ws_terminal(
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

    ws.on_upgrade(move |socket| handle_ws(socket, session_id, state))
}

async fn handle_ws(socket: WebSocket, session_id: String, state: Arc<AppState>) {
    let mut output_rx = match state.sessions.take_output_rx(&session_id) {
        Some(rx) => rx,
        None => {
            tracing::error!("Session {} not found or already connected", session_id);
            return;
        }
    };

    let (mut ws_sink, mut ws_stream) = socket.split();
    let logger = state.logger.clone();

    // Replay scrollback history first
    let scrollback = state.sessions.get_scrollback(&session_id);
    for b64 in scrollback {
        let msg = serde_json::json!({"type": "output", "data": b64});
        if ws_sink
            .send(Message::Text(msg.to_string().into()))
            .await
            .is_err()
        {
            state.sessions.return_output_rx(&session_id, output_rx);
            return;
        }
    }

    // Single select! loop — ensures output_rx is always returned on disconnect
    loop {
        tokio::select! {
            data = output_rx.recv() => {
                match data {
                    Some(bytes) => {
                        let b64 = base64::Engine::encode(
                            &base64::engine::general_purpose::STANDARD, &bytes);

                        // Log output
                        if let Some(ref log) = logger {
                            log.log_pty_output(&session_id, &b64);
                        }

                        // Push to scrollback buffer for future reconnects
                        state.sessions.push_scrollback(&session_id, b64.clone());

                        let msg = serde_json::json!({"type": "output", "data": b64});
                        if ws_sink
                            .send(Message::Text(msg.to_string().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    None => break, // PTY closed
                }
            }

            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(client_msg) = serde_json::from_str::<ClientMsg>(&text) {
                            match client_msg {
                                ClientMsg::Input { data } => {
                                    if let Some(ref log) = logger {
                                        log.log_pty_input(&session_id, &data);
                                    }
                                    if let Ok(bytes) = base64::Engine::decode(
                                        &base64::engine::general_purpose::STANDARD,
                                        &data,
                                    ) {
                                        let _ = state.sessions.write_to_session(&session_id, &bytes);
                                    }
                                }
                                ClientMsg::Resize { cols, rows } => {
                                    let _ = state.sessions.resize_session(&session_id, cols, rows);
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Binary(data))) => {
                        let _ = state.sessions.write_to_session(&session_id, &data);
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }

    // Always return the receiver so the session can be reconnected
    state.sessions.return_output_rx(&session_id, output_rx);
    tracing::info!("WebSocket disconnected for session {}", session_id);
}
