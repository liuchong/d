//! MCP client implementation

use crate::types::*;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

/// MCP client trait
#[async_trait]
pub trait McpClient: Send + Sync {
    /// Initialize connection
    async fn initialize(&mut self) -> Result<InitializeResponse>;

    /// List available tools
    async fn list_tools(&self, cursor: Option<String>) -> Result<ListToolsResult>;

    /// Call a tool
    async fn call_tool(&self, name: &str, arguments: Option<serde_json::Value>) -> Result<CallToolResult>;

    /// List resources
    async fn list_resources(&self, cursor: Option<String>) -> Result<ListResourcesResult>;

    /// Read resource
    async fn read_resource(&self, uri: &str) -> Result<ResourceContents>;

    /// List prompts
    async fn list_prompts(&self, cursor: Option<String>) -> Result<ListPromptsResult>;

    /// Get prompt
    async fn get_prompt(&self, name: &str, arguments: Option<HashMap<String, String>>) -> Result<Vec<PromptMessage>>;
}

use std::collections::HashMap;

/// HTTP MCP client
pub struct HttpMcpClient {
    base_url: String,
    client: reqwest::Client,
    request_id: AtomicU64,
    capabilities: Option<Capabilities>,
}

impl HttpMcpClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            client: reqwest::Client::new(),
            request_id: AtomicU64::new(1),
            capabilities: None,
        }
    }

    fn next_id(&self) -> u64 {
        self.request_id.fetch_add(1, Ordering::SeqCst)
    }

    async fn send_request(&self, method: &str, params: Option<serde_json::Value>) -> Result<serde_json::Value> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(self.next_id()),
            method: method.to_string(),
            params,
        };

        let response: JsonRpcResponse = self.client
            .post(&self.base_url)
            .json(&request)
            .send()
            .await?
            .json()
            .await?;

        match response.result {
            JsonRpcResult::Result(value) => Ok(value),
            JsonRpcResult::Error(e) => Err(anyhow!("MCP error {}: {}", e.code, e.message)),
        }
    }
}

#[async_trait]
impl McpClient for HttpMcpClient {
    async fn initialize(&mut self) -> Result<InitializeResponse> {
        let request = InitializeRequest {
            protocol_version: MCP_VERSION.to_string(),
            capabilities: Capabilities {
                tools: Some(ToolsCapability { list_changed: true }),
                resources: Some(ResourcesCapability { subscribe: true, list_changed: true }),
                prompts: Some(PromptsCapability { list_changed: true }),
            },
            client_info: Implementation {
                name: "d-chat".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };

        let result = self.send_request("initialize", Some(json!(request))).await?;
        let response: InitializeResponse = serde_json::from_value(result)?;
        self.capabilities = Some(response.capabilities.clone());
        
        tracing::info!("MCP initialized with server: {} {}", 
            response.server_info.name, 
            response.server_info.version
        );
        
        Ok(response)
    }

    async fn list_tools(&self, cursor: Option<String>) -> Result<ListToolsResult> {
        let params = cursor.map(|c| json!({ "cursor": c }));
        let result = self.send_request("tools/list", params).await?;
        Ok(serde_json::from_value(result)?)
    }

    async fn call_tool(&self, name: &str, arguments: Option<serde_json::Value>) -> Result<CallToolResult> {
        let request = CallToolRequest {
            name: name.to_string(),
            arguments,
        };
        let result = self.send_request("tools/call", Some(json!(request))).await?;
        Ok(serde_json::from_value(result)?)
    }

    async fn list_resources(&self, cursor: Option<String>) -> Result<ListResourcesResult> {
        let params = cursor.map(|c| json!({ "cursor": c }));
        let result = self.send_request("resources/list", params).await?;
        Ok(serde_json::from_value(result)?)
    }

    async fn read_resource(&self, uri: &str) -> Result<ResourceContents> {
        let params = json!({ "uri": uri });
        let result = self.send_request("resources/read", Some(params)).await?;
        Ok(serde_json::from_value(result)?)
    }

    async fn list_prompts(&self, cursor: Option<String>) -> Result<ListPromptsResult> {
        let params = cursor.map(|c| json!({ "cursor": c }));
        let result = self.send_request("prompts/list", params).await?;
        Ok(serde_json::from_value(result)?)
    }

    async fn get_prompt(&self, name: &str, arguments: Option<HashMap<String, String>>) -> Result<Vec<PromptMessage>> {
        let mut params = json!({ "name": name });
        if let Some(args) = arguments {
            params["arguments"] = json!(args);
        }
        let result = self.send_request("prompts/get", Some(params)).await?;
        let messages: Vec<PromptMessage> = serde_json::from_value(result)?;
        Ok(messages)
    }
}

/// Stdio MCP client (for subprocess communication)
pub struct StdioMcpClient {
    // TODO: Implement stdio transport for local MCP servers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_client_creation() {
        let client = HttpMcpClient::new("http://localhost:8080/mcp");
        assert_eq!(client.base_url, "http://localhost:8080/mcp");
    }

    #[tokio::test]
    async fn test_tool_serialization() {
        let tool = Tool {
            name: "test".to_string(),
            description: "Test tool".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        };

        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains("test"));
        assert!(json.contains("Test tool"));
    }
}
