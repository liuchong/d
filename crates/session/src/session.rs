use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use llm::Message;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub messages: Vec<Message>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl Session {
    pub fn new(id: impl Into<String>) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: id.into(),
            title: "New Session".to_string(),
            messages: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
        self.updated_at = chrono::Utc::now();
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.updated_at = chrono::Utc::now();
    }
}

pub struct SessionManager {
    sessions: RwLock<HashMap<String, Session>>,
    storage_dir: PathBuf,
}

impl SessionManager {
    pub fn new() -> anyhow::Result<Self> {
        let storage_dir = dirs::data_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot find data directory"))?
            .join("d")
            .join("sessions");

        std::fs::create_dir_all(&storage_dir)?;

        Ok(Self {
            sessions: RwLock::new(HashMap::new()),
            storage_dir,
        })
    }

    pub fn create(&self) -> Session {
        let id = uuid::Uuid::new_v4().to_string();
        let session = Session::new(&id);
        self.sessions.write().unwrap().insert(id.clone(), session.clone());
        session
    }

    pub fn get(&self, id: &str) -> Option<Session> {
        self.sessions.read().unwrap().get(id).cloned()
    }

    pub fn get_or_create(&self, id: &str) -> Session {
        if let Some(session) = self.get(id) {
            session
        } else {
            let session = Session::new(id);
            self.sessions.write().unwrap().insert(id.to_string(), session.clone());
            session
        }
    }

    pub fn get_mut(&self, id: &str) -> Option<std::sync::RwLockWriteGuard<Session>> {
        // This API is tricky with RwLock, for now return cloned
        self.sessions.write().unwrap().get_mut(id).map(|s| {
            // Can't return a guard directly, we'd need a different pattern
            // For now, just return None and require using update() pattern
let _ = s;
            None
        })?
    }

    pub fn update<F>(&self, id: &str, f: F) -> anyhow::Result<()>
    where
        F: FnOnce(&mut Session),
    {
        let mut sessions = self.sessions.write().unwrap();
        if let Some(session) = sessions.get_mut(id) {
            f(session);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Session not found: {}", id))
        }
    }

    pub fn list(&self) -> Vec<Session> {
        self.sessions.read().unwrap().values().cloned().collect()
    }

    pub fn delete(&self, id: &str) -> Option<Session> {
        self.sessions.write().unwrap().remove(id)
    }

    pub fn save(&self, session: &Session) -> anyhow::Result<()> {
        let file_path = self.storage_dir.join(format!("{}.json", session.id));
        let json = serde_json::to_string_pretty(session)?;
        std::fs::write(file_path, json)?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> anyhow::Result<Option<Session>> {
        let file_path = self.storage_dir.join(format!("{}.json", id));
        if file_path.exists() {
            let json = std::fs::read_to_string(file_path)?;
            let session: Session = serde_json::from_str(&json)?;
            self.sessions.write().unwrap().insert(id.to_string(), session.clone());
            Ok(Some(session))
        } else {
            Ok(None)
        }
    }
}
