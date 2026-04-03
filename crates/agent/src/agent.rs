use llm::{AiClient, Message, Tool};
use session::SessionManager;
use memory::MemoryStore;
use security::ApprovalSystem;
use std::sync::Arc;

/// The main agent struct that orchestrates AI interactions
pub struct Agent {
    client: AiClient,
    session_manager: Arc<SessionManager>,
    memory: Option<Arc<dyn MemoryStore>>,
    approval: Arc<ApprovalSystem>,
    tools: Vec<Tool>,
}

impl Agent {
    pub fn new(
        client: AiClient,
        session_manager: Arc<SessionManager>,
        approval: Arc<ApprovalSystem>,
    ) -> Self {
        Self {
            client,
            session_manager,
            memory: None,
            approval,
            tools: Vec::new(),
        }
    }

    pub fn with_memory(mut self, memory: Arc<dyn MemoryStore>) -> Self {
        self.memory = Some(memory);
        self
    }

    pub fn with_tools(mut self, tools: Vec<Tool>) -> Self {
        self.tools = tools;
        self
    }

    /// Run a single turn conversation
    pub async fn chat(&self, session_id: &str, user_input: &str) -> anyhow::Result<String> {
        // Get or create session
        let session = self.session_manager.get_or_create(session_id);
        
        // Clone messages for LLM call
        let mut messages = session.messages.clone();
        
        // Add user message
        messages.push(Message::user(user_input));
        
        // Retrieve relevant memories if available
        if let Some(ref memory) = self.memory {
            // TODO: Query memories and inject into context
            let _ = memory;
        }
        
        // Call LLM - convert to ChatMessage
        let chat_messages: Vec<llm::client::ChatMessage> = messages.iter().map(|m| {
            llm::client::ChatMessage {
                role: format!("{:?}", m.role).to_lowercase(),
                content: m.content.clone(),
            }
        }).collect();
        
        let response = self.client.chat(chat_messages).await.map_err(|e| anyhow::anyhow!("{}", e))?;
        
        // Update session with new messages
        self.session_manager.update(session_id, |s| {
            s.add_message(Message::user(user_input));
            s.add_message(Message::assistant(&response));
        })?;
        
        // Save session
        let updated = self.session_manager.get(session_id).unwrap();
        self.session_manager.save(&updated)?;
        
        Ok(response)
    }

    /// Run agent loop with tool calling
    pub async fn run_with_tools(&self, _goal: &str) -> anyhow::Result<String> {
        // TODO: Implement ReAct loop
        // 1. Plan next action
        // 2. Execute tool (with approval)
        // 3. Observe result
        // 4. Repeat until complete
        todo!("ReAct loop implementation")
    }
}
