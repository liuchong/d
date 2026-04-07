//! WebSocket support for streaming chat

use axum::{
    extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
    extract::State,
    response::Response,
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};


use crate::server::ServerState;

/// WebSocket message types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WsChatMessage {
    #[serde(rename = "user")]
    User { content: String, session_id: Option<String> },
    #[serde(rename = "assistant")]
    Assistant { content: String, delta: bool },
    #[serde(rename = "tool_call")]
    ToolCall { name: String, arguments: serde_json::Value },
    #[serde(rename = "tool_result")]
    ToolResult { name: String, result: String },
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(rename = "connected")]
    Connected { session_id: String },
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "pong")]
    Pong,
}

/// WebSocket handler
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(_state): State<ServerState>,
) -> Response {
    ws.on_upgrade(handle_socket)
}

/// Handle WebSocket connection
async fn handle_socket(mut socket: WebSocket) {
    // Generate session ID for this connection
    let session_id = uuid::Uuid::new_v4().to_string();
    
    // Send connection confirmation
    let connected_msg = WsChatMessage::Connected {
        session_id: session_id.clone(),
    };
    
    if let Ok(json) = serde_json::to_string(&connected_msg) {
        if socket.send(WsMessage::Text(json.into())).await.is_err() {
            return;
        }
    }

    // Message loop
    while let Some(msg) = socket.recv().await {
        match msg {
            Ok(WsMessage::Text(text)) => {
                // Parse incoming message
                match serde_json::from_str::<WsChatMessage>(&text) {
                    Ok(WsChatMessage::User { content, session_id: _ }) => {
                        // Echo back for now (placeholder for actual chat)
                        let response = WsChatMessage::Assistant {
                            content: format!("Received: {}", content),
                            delta: false,
                        };
                        
                        if let Ok(json) = serde_json::to_string(&response) {
                            if socket.send(WsMessage::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Ok(WsChatMessage::Ping) => {
                        let pong = WsChatMessage::Pong;
                        if let Ok(json) = serde_json::to_string(&pong) {
                            if socket.send(WsMessage::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        let error = WsChatMessage::Error {
                            message: format!("Invalid message: {}", e),
                        };
                        if let Ok(json) = serde_json::to_string(&error) {
                            let _ = socket.send(WsMessage::Text(json.into())).await;
                        }
                    }
                    _ => {}
                }
            }
            Ok(WsMessage::Close(_)) => {
                break;
            }
            Err(_) => {
                break;
            }
            _ => {}
        }
    }
}

/// Create WebSocket routes
pub fn ws_routes() -> Router<ServerState> {
    Router::new().route("/ws/chat", get(ws_handler))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ws_message_serialization() {
        let msg = WsChatMessage::User {
            content: "Hello".to_string(),
            session_id: Some("test".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("user"));
        assert!(json.contains("Hello"));

        // Deserialize
        let decoded: WsChatMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            WsChatMessage::User { content, .. } => assert_eq!(content, "Hello"),
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_ws_assistant_message() {
        let msg = WsChatMessage::Assistant {
            content: "Hello back".to_string(),
            delta: true,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("assistant"));
        assert!(json.contains("delta"));
    }

    #[test]
    fn test_ws_connected_message() {
        let msg = WsChatMessage::Connected {
            session_id: "test-id".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("connected"));
        assert!(json.contains("test-id"));
    }

    #[test]
    fn test_ping_pong() {
        let ping = WsChatMessage::Ping;
        let ping_json = serde_json::to_string(&ping).unwrap();
        assert!(ping_json.contains("ping"));

        let pong = WsChatMessage::Pong;
        let pong_json = serde_json::to_string(&pong).unwrap();
        assert!(pong_json.contains("pong"));
    }
}
