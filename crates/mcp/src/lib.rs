//! MCP (Model Context Protocol) implementation
//!
//! Model Context Protocol is an open protocol that standardizes how applications
//! provide context to LLMs. This crate provides both client and server implementations.
//!
//! ## Example
//!
//! ```rust
//! use mcp::{McpClient, HttpMcpClient};
//!
//! # async fn example() -> anyhow::Result<()> {
//! let mut client = HttpMcpClient::new("http://localhost:8080/mcp");
//! let init = client.initialize().await?;
//! println!("Connected to: {}", init.server_info.name);
//! # Ok(())
//! # }
//! ```

pub mod client;
pub mod types;

pub use client::{HttpMcpClient, McpClient};
pub use types::*;
