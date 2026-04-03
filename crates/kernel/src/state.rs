use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub type SharedState = Arc<RwLock<State>>;

#[derive(Default)]
pub struct State {
    pub data: HashMap<String, serde_json::Value>,
}

impl State {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.data.get(key)
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) {
        self.data.insert(key.into(), value.into());
    }
}
