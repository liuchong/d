//! HTTP server with file serving and WebSocket support

pub mod api;
pub mod server;
pub mod utils;
pub mod websocket;

pub use api::api_routes;
pub use server::{create_app, start, ServerState};
pub use websocket::{ws_routes, WsChatMessage};
pub use utils::{
    decode_path, encode_path, format_http_date, format_size, guess_mime_type,
    html_escape,
};
