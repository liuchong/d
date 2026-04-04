//! HTTP API endpoints for chat

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json, Sse},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::server::ServerState;

/// Chat request
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub session_id: Option<String>,
    pub message: String,
    pub stream: Option<bool>,
}

/// Chat response
#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub session_id: String,
    pub response: String,
    pub tool_calls: Option<Vec<serde_json::Value>>,
}

/// Session info
#[derive(Debug, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub message_count: usize,
    pub created_at: String,
}

/// Error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

/// Create API routes
pub fn api_routes() -> Router<ServerState> {
    Router::new()
        .route("/api/chat", post(chat_handler))
        .route("/api/sessions", get(list_sessions))
        .route("/api/sessions/{id}", get(get_session))
        .route("/api/sessions/{id}", post(create_session))
        .route("/api/sessions/{id}/clear", post(clear_session))
        .route("/api/health", get(health_check))
}

/// Health check endpoint
async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// Chat handler
async fn chat_handler(
    State(_state): State<ServerState>,
    Json(request): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, (StatusCode, Json<ErrorResponse>)> {
    let session_id = request.session_id.unwrap_or_else(|| {
        uuid::Uuid::new_v4().to_string()
    });

    // TODO: Integrate with agent
    // For now, return a placeholder response
    let response = ChatResponse {
        session_id,
        response: format!("Received: {}", request.message),
        tool_calls: None,
    };

    Ok(Json(response))
}

/// List all sessions
async fn list_sessions(
    State(_state): State<ServerState>,
) -> Json<Vec<SessionInfo>> {
    // TODO: Get from session manager
    Json(vec![])
}

/// Get a specific session
async fn get_session(
    State(_state): State<ServerState>,
    Path(id): Path<String>,
) -> Result<Json<SessionInfo>, (StatusCode, Json<ErrorResponse>)> {
    // TODO: Get from session manager
    Err((
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            error: format!("Session {} not found", id),
        }),
    ))
}

/// Create a new session
async fn create_session(
    State(_state): State<ServerState>,
    Path(_id): Path<String>,
) -> Json<SessionInfo> {
    let id = uuid::Uuid::new_v4().to_string();
    Json(SessionInfo {
        id,
        title: "New Session".to_string(),
        message_count: 0,
        created_at: chrono::Utc::now().to_rfc3339(),
    })
}

/// Clear a session
async fn clear_session(
    State(_state): State<ServerState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    // TODO: Clear session
    Ok(Json(serde_json::json!({
        "success": true,
        "session_id": id,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_request_deserialization() {
        let json = r#"{
            "message": "Hello",
            "stream": false
        }"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.message, "Hello");
        assert_eq!(req.stream, Some(false));
        assert!(req.session_id.is_none());
    }

    #[test]
    fn test_chat_response_serialization() {
        let resp = ChatResponse {
            session_id: "test-id".to_string(),
            response: "Hello back".to_string(),
            tool_calls: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("test-id"));
        assert!(json.contains("Hello back"));
    }

    #[test]
    fn test_session_info_serialization() {
        let info = SessionInfo {
            id: "id".to_string(),
            title: "Title".to_string(),
            message_count: 5,
            created_at: "2024-01-01".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("id"));
        assert!(json.contains("Title"));
    }
}
