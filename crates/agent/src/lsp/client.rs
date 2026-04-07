//! LSP client implementation

use super::types::*;
use super::LspError;
use serde_json::Value;
use std::collections::HashMap;


use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, error, info, trace};

/// Pending request
struct PendingRequest {
    sender: oneshot::Sender<Result<Value, LspError>>,
}

/// LSP client
pub struct LspClient {
    process: Child,
    stdin: ChildStdin,
    request_id: AtomicI64,
    pending: Arc<Mutex<HashMap<i64, PendingRequest>>>,
    notification_tx: mpsc::UnboundedSender<Message>,
    root_uri: String,
}

/// Client handle for making requests
#[derive(Clone)]
pub struct LspClientHandle {
    request_tx: mpsc::UnboundedSender<ClientRequest>,
    root_uri: String,
}

/// Client request
enum ClientRequest {
    Request {
        method: String,
        params: Option<Value>,
        response: oneshot::Sender<Result<Value, LspError>>,
    },
    Notification {
        method: String,
        params: Option<Value>,
    },
    Shutdown,
}

/// Internal message type
enum Message {
    Request(Request),
    Response(Response),
}

impl LspClient {
    /// Create and initialize a new LSP client
    pub async fn new(
        command: &str,
        args: &[String],
        root_uri: &str,
    ) -> Result<Self, LspError> {
        info!("Starting LSP server: {}", command);

        let mut process = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| LspError::ServerError(format!("Failed to start {}: {}", command, e)))?;

        let stdin = process.stdin.take().ok_or_else(|| {
            LspError::ServerError("Failed to get stdin".to_string())
        })?;

        let stdout = process.stdout.take().ok_or_else(|| {
            LspError::ServerError("Failed to get stdout".to_string())
        })?;

        let pending = Arc::new(Mutex::new(HashMap::<i64, PendingRequest>::new()));
        let (notification_tx, mut notification_rx) = mpsc::unbounded_channel();

        // Spawn reader task
        let pending_for_reader = pending.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();

            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => {
                        debug!("LSP server stdout closed");
                        break;
                    }
                    Ok(_) => {
                        if line.trim().is_empty() {
                            continue;
                        }

                        // Parse Content-Length header
                        let content_length = line
                            .strip_prefix("Content-Length: ")
                            .and_then(|s| s.trim().parse::<usize>().ok());

                        if let Some(len) = content_length {
                            // Read empty line
                            line.clear();
                            let _ = reader.read_line(&mut line).await;

                            // Read content
                            let mut content = vec![0u8; len];
                            if let Err(e) = reader.read_exact(&mut content).await {
                                error!("Failed to read content: {}", e);
                                continue;
                            }

                            let content_str = String::from_utf8_lossy(&content);
                            trace!("LSP recv: {}", content_str);

                            // Try to parse as response first
                            if let Ok(response) = serde_json::from_str::<Response>(&content_str) {
                                let id = response.id;
                                if let Some(id) = id {
                                    let mut pending = pending_for_reader.lock().await;
                                    if let Some(req) = pending.remove(&id) {
                                        let result = if let Some(error) = response.error {
                                            Err(LspError::Lsp {
                                                code: error.code,
                                                message: error.message,
                                            })
                                        } else {
                                            Ok(response.result.unwrap_or(Value::Null))
                                        };
                                        let _ = req.sender.send(result);
                                    }
                                }
                            } else if let Ok(request) = serde_json::from_str::<Request>(&content_str) {
                                // Server-to-client request (not common)
                                debug!("Received server request: {}", request.method);
                            } else {
                                trace!("Unparseable LSP message: {}", content_str);
                            }
                        }
                    }
                    Err(e) => {
                        error!("Error reading from LSP server: {}", e);
                        break;
                    }
                }
            }

            // Clean up pending requests
            let mut pending = pending_for_reader.lock().await;
            for (_, req) in pending.drain() {
                let _ = req.sender.send(Err(LspError::ServerError("Server disconnected".to_string())));
            }
        });

        // Spawn stderr reader
        if let Some(stderr) = process.stderr.take() {
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("LSP stderr: {}", line);
                }
            });
        }

        Ok(Self {
            process,
            stdin,
            request_id: AtomicI64::new(1),
            pending,
            notification_tx,
            root_uri: root_uri.to_string(),
        })
    }

    /// Initialize the client
    pub async fn initialize(&mut self) -> Result<InitializeResult, LspError> {
        let params = InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: self.root_uri.clone(),
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    synchronization: Some(SynchronizationCapability {
                        dynamic_registration: Some(false),
                    }),
                    completion: Some(CompletionCapability {
                        dynamic_registration: Some(false),
                        completion_item: Some(CompletionItemCapability {
                            snippet_support: Some(true),
                        }),
                    }),
                    hover: Some(HoverCapability {
                        dynamic_registration: Some(false),
                    }),
                    definition: Some(DefinitionCapability {
                        dynamic_registration: Some(false),
                        link_support: Some(true),
                    }),
                    references: Some(ReferencesCapability {
                        dynamic_registration: Some(false),
                    }),
                    document_symbol: Some(DocumentSymbolCapability {
                        dynamic_registration: Some(false),
                        hierarchical_document_symbol_support: Some(true),
                    }),
                    code_action: Some(CodeActionCapability {
                        dynamic_registration: Some(false),
                    }),
                }),
                workspace: Some(WorkspaceClientCapabilities {
                    workspace_folders: Some(false),
                    configuration: Some(false),
                }),
            },
            initialization_options: None,
        };

        let result = self
            .request("initialize", Some(serde_json::to_value(params)?))
            .await?;

        let init_result: InitializeResult = serde_json::from_value(result)?;

        // Send initialized notification
        self.notify("initialized", Some(serde_json::json!({})))
            .await?;

        info!("LSP client initialized: {:?}", init_result.server_info);

        Ok(init_result)
    }

    /// Spawn the client and return a handle
    pub async fn spawn(mut self) -> Result<LspClientHandle, LspError> {
        // Initialize first
        let _ = self.initialize().await?;

        let root_uri = self.root_uri.clone();
        let (request_tx, mut request_rx) = mpsc::unbounded_channel::<ClientRequest>();

        // Spawn request handler
        tokio::spawn(async move {
            while let Some(request) = request_rx.recv().await {
                match request {
                    ClientRequest::Request { method, params, response } => {
                        let result = self.request(&method, params).await;
                        let _ = response.send(result);
                    }
                    ClientRequest::Notification { method, params } => {
                        let _ = self.notify(&method, params).await;
                    }
                    ClientRequest::Shutdown => {
                        let _ = self.shutdown().await;
                        break;
                    }
                }
            }
        });

        Ok(LspClientHandle { request_tx, root_uri })
    }

    /// Send a request and wait for response
    async fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value, LspError> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        
        let request = Request::new(id, method.to_string(), params);
        let json = serde_json::to_string(&request)?;

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, PendingRequest { sender: tx });
        }

        self.send_message(&json).await?;

        match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(LspError::Cancelled),
            Err(_) => {
                let mut pending = self.pending.lock().await;
                pending.remove(&id);
                Err(LspError::Timeout)
            }
        }
    }

    /// Send a notification
    async fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), LspError> {
        let notification = Request::notification(method.to_string(), params);
        let json = serde_json::to_string(&notification)?;
        self.send_message(&json).await
    }

    /// Send raw message
    async fn send_message(&mut self, json: &str) -> Result<(), LspError> {
        let message = format!("Content-Length: {}\r\n\r\n{}", json.len(), json);
        trace!("LSP send: {}", json);
        self.stdin.write_all(message.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    /// Shutdown the client
    async fn shutdown(&mut self) -> Result<(), LspError> {
        info!("Shutting down LSP client");

        // Send shutdown request
        let _ = self.request("shutdown", None).await;

        // Send exit notification
        let _ = self.notify("exit", None).await;

        // Kill the process
        let _ = self.process.kill().await;

        Ok(())
    }
}

impl LspClientHandle {
    /// Get root URI
    pub fn root_uri(&self) -> &str {
        &self.root_uri
    }

    /// Send a request
    async fn request(&self, method: &str, params: Option<Value>) -> Result<Value, LspError> {
        let (tx, rx) = oneshot::channel();
        self.request_tx
            .send(ClientRequest::Request {
                method: method.to_string(),
                params,
                response: tx,
            })
            .map_err(|_| LspError::ServerError("Client closed".to_string()))?;

        rx.await
            .map_err(|_| LspError::ServerError("Request cancelled".to_string()))?
    }

    /// Send a notification
    async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), LspError> {
        self.request_tx
            .send(ClientRequest::Notification {
                method: method.to_string(),
                params,
            })
            .map_err(|_| LspError::ServerError("Client closed".to_string()))
    }

    /// Open a text document
    pub async fn did_open(
        &self,
        uri: &str,
        language_id: &str,
        version: i32,
        text: &str,
    ) -> Result<(), LspError> {
        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.to_string(),
                language_id: language_id.to_string(),
                version,
                text: text.to_string(),
            },
        };
        self.notify("textDocument/didOpen", Some(serde_json::to_value(params)?))
            .await
    }

    /// Change a text document
    pub async fn did_change(
        &self,
        uri: &str,
        version: i32,
        changes: Vec<TextDocumentContentChangeEvent>,
    ) -> Result<(), LspError> {
        let params = DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.to_string(),
                version,
            },
            content_changes: changes,
        };
        self.notify(
            "textDocument/didChange",
            Some(serde_json::to_value(params)?),
        )
        .await
    }

    /// Close a text document
    pub async fn did_close(&self, uri: &str) -> Result<(), LspError> {
        let params = DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier {
                uri: uri.to_string(),
            },
        };
        self.notify("textDocument/didClose", Some(serde_json::to_value(params)?))
            .await
    }

    /// Get completions at position
    pub async fn completion(&self, uri: &str, line: u32, character: u32) -> Result<CompletionList, LspError> {
        let params = CompletionParams {
            text_document: TextDocumentIdentifier {
                uri: uri.to_string(),
            },
            position: Position { line, character },
        };
        let result = self.request("textDocument/completion", Some(serde_json::to_value(params)?)).await?;
        
        // Completion can return either CompletionList or Vec<CompletionItem>
        if let Ok(list) = serde_json::from_value::<CompletionList>(result.clone()) {
            Ok(list)
        } else if let Ok(items) = serde_json::from_value::<Vec<CompletionItem>>(result) {
            Ok(CompletionList {
                is_incomplete: false,
                items,
            })
        } else {
            Ok(CompletionList {
                is_incomplete: false,
                items: vec![],
            })
        }
    }

    /// Get hover information at position
    pub async fn hover(&self, uri: &str, line: u32, character: u32) -> Result<Option<Hover>, LspError> {
        let params = HoverParams {
            text_document: TextDocumentIdentifier {
                uri: uri.to_string(),
            },
            position: Position { line, character },
        };
        let result = self.request("textDocument/hover", Some(serde_json::to_value(params)?)).await?;
        
        if result.is_null() {
            Ok(None)
        } else {
            Ok(Some(serde_json::from_value(result)?))
        }
    }

    /// Go to definition
    pub async fn goto_definition(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Vec<Location>, LspError> {
        let params = DefinitionParams {
            text_document: TextDocumentIdentifier {
                uri: uri.to_string(),
            },
            position: Position { line, character },
        };
        let result = self
            .request("textDocument/definition", Some(serde_json::to_value(params)?))
            .await?;

        // Definition can return Location, Vec<Location>, or null
        if result.is_null() {
            Ok(vec![])
        } else if let Ok(location) = serde_json::from_value::<Location>(result.clone()) {
            Ok(vec![location])
        } else if let Ok(locations) = serde_json::from_value::<Vec<Location>>(result) {
            Ok(locations)
        } else {
            Ok(vec![])
        }
    }

    /// Find references
    pub async fn find_references(
        &self,
        uri: &str,
        line: u32,
        character: u32,
        include_declaration: bool,
    ) -> Result<Vec<Location>, LspError> {
        let params = ReferenceParams {
            text_document: TextDocumentIdentifier {
                uri: uri.to_string(),
            },
            position: Position { line, character },
            context: ReferenceContext {
                include_declaration,
            },
        };
        let result = self
            .request("textDocument/references", Some(serde_json::to_value(params)?))
            .await?;

        if result.is_null() {
            Ok(vec![])
        } else {
            Ok(serde_json::from_value(result)?)
        }
    }

    /// Get document symbols
    pub async fn document_symbol(&self, uri: &str) -> Result<Vec<DocumentSymbol>, LspError> {
        let params = DocumentSymbolParams {
            text_document: TextDocumentIdentifier {
                uri: uri.to_string(),
            },
        };
        let result = self
            .request("textDocument/documentSymbol", Some(serde_json::to_value(params)?))
            .await?;

        if result.is_null() {
            Ok(vec![])
        } else {
            // Can return either Vec<DocumentSymbol> or Vec<DocumentSymbolInformation>
            // For simplicity, we just try Vec<DocumentSymbol>
            serde_json::from_value(result).map_err(Into::into)
        }
    }

    /// Get workspace symbols
    pub async fn workspace_symbol(&self, query: &str) -> Result<Vec<DocumentSymbol>, LspError> {
        let params = WorkspaceSymbolParams {
            query: query.to_string(),
        };
        let result = self
            .request("workspace/symbol", Some(serde_json::to_value(params)?))
            .await?;

        if result.is_null() {
            Ok(vec![])
        } else {
            serde_json::from_value(result).map_err(Into::into)
        }
    }

    /// Shutdown the client
    pub async fn shutdown(&self) -> Result<(), LspError> {
        self.request_tx
            .send(ClientRequest::Shutdown)
            .map_err(|_| LspError::ServerError("Client closed".to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completion_item_kind() {
        assert_eq!(CompletionItemKind::Function.as_str(), "function");
        assert_eq!(CompletionItemKind::Class.as_str(), "class");
        assert_eq!(CompletionItemKind::Variable.as_str(), "variable");
    }
}
