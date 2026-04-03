use std::sync::Arc;
use tokio::sync::RwLock;

use crate::{config::Config, error::Result, module::Module, state::SharedState};

pub struct Daemon {
    config: Config,
    state: SharedState,
    modules: Vec<Box<dyn Module>>,
}

impl Daemon {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            state: Arc::new(RwLock::new(crate::state::State::default())),
            modules: Vec::new(),
        }
    }

    pub fn with_module(mut self, module: Box<dyn Module>) -> Self {
        self.modules.push(module);
        self
    }

    pub async fn start(&self) -> Result<()> {
        for module in &self.modules {
            module.init(self.state.clone()).await?;
        }
        
        for module in &self.modules {
            module.start().await?;
        }
        
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<()> {
        for module in &self.modules {
            module.shutdown().await?;
        }
        Ok(())
    }
}

pub struct DaemonBuilder {
    config: Config,
    modules: Vec<Box<dyn Module>>,
}

impl DaemonBuilder {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            modules: Vec::new(),
        }
    }

    pub fn add_module(mut self, module: Box<dyn Module>) -> Self {
        self.modules.push(module);
        self
    }

    pub fn build(self) -> Daemon {
        Daemon {
            config: self.config,
            state: Arc::new(RwLock::new(crate::state::State::default())),
            modules: self.modules,
        }
    }
}
