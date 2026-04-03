use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::error::Result;
use crate::state::State;

pub type SharedState = Arc<RwLock<State>>;

#[async_trait]
pub trait Module: Send + Sync {
    fn name(&self) -> &str;
    
    async fn init(&self, state: SharedState) -> Result<()> {
        let _ = state;
        Ok(())
    }
    
    async fn start(&self) -> Result<()> {
        Ok(())
    }
    
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

pub struct ModuleContext {
    pub name: String,
    pub state: SharedState,
}

impl ModuleContext {
    pub fn new(name: impl Into<String>, state: SharedState) -> Self {
        Self {
            name: name.into(),
            state,
        }
    }
}
