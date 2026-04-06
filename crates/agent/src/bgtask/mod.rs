//! Background task management system
//!
//! Manages long-running operations:
//! - Shell commands
//! - File watching
//! - Agent tasks
//! - HTTP requests

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

/// Unique task ID
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub u64);

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

use std::fmt;

impl TaskId {
    pub fn new(id: u64) -> Self {
        Self(id)
    }
}

/// Task types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    /// Shell command execution
    ShellCommand {
        command: String,
        working_dir: Option<String>,
    },
    /// Agent task
    AgentTask {
        prompt: String,
        context: HashMap<String, String>,
    },
    /// File watching
    FileWatch {
        path: String,
        pattern: Option<String>,
    },
    /// HTTP request
    HttpRequest {
        url: String,
        method: String,
        body: Option<String>,
    },
}

/// Task status
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Task is pending
    Pending,
    /// Task is running
    Running,
    /// Task completed successfully
    Completed,
    /// Task failed
    Failed,
    /// Task was cancelled
    Cancelled,
}

impl TaskStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, TaskStatus::Pending | TaskStatus::Running)
    }

    pub fn name(&self) -> &'static str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::Running => "running",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        }
    }
}

/// Background task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundTask {
    pub id: TaskId,
    pub task_type: TaskType,
    pub status: TaskStatus,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub result: Option<TaskResult>,
    pub auto_restart: bool,
    pub max_restarts: u32,
    pub restart_count: u32,
}

/// Task execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub success: bool,
    pub output: String,
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl BackgroundTask {
    /// Create a new background task
    pub fn new(id: TaskId, task_type: TaskType) -> Self {
        Self {
            id,
            task_type,
            status: TaskStatus::Pending,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            result: None,
            auto_restart: false,
            max_restarts: 3,
            restart_count: 0,
        }
    }

    /// Set auto-restart
    pub fn with_auto_restart(mut self, enabled: bool) -> Self {
        self.auto_restart = enabled;
        self
    }

    /// Get duration since creation
    pub fn duration(&self) -> std::time::Duration {
        let end = self.completed_at.unwrap_or_else(|| Utc::now());
        (end - self.created_at).to_std().unwrap_or_default()
    }

    /// Format for display
    pub fn format_summary(&self) -> String {
        let status_icon = match self.status {
            TaskStatus::Pending => "⏳",
            TaskStatus::Running => "▶️",
            TaskStatus::Completed => "✅",
            TaskStatus::Failed => "❌",
            TaskStatus::Cancelled => "⏹️",
        };

        let type_name = match &self.task_type {
            TaskType::ShellCommand { command, .. } => format!("shell: {}", command.split_whitespace().next().unwrap_or(command)),
            TaskType::AgentTask { .. } => "agent".to_string(),
            TaskType::FileWatch { path, .. } => format!("watch: {}", path),
            TaskType::HttpRequest { url, .. } => format!("http: {}", url),
        };

        format!("{} {} - {} [{}]", status_icon, self.id, type_name, self.status.name())
    }
}

/// Task manager
pub struct TaskManager {
    next_id: AtomicU64,
    tasks: Arc<RwLock<HashMap<TaskId, BackgroundTask>>>,
    running: Arc<RwLock<HashMap<TaskId, RunningTask>>>,
    event_tx: mpsc::UnboundedSender<TaskEvent>,
}

/// Running task handle
struct RunningTask {
    handle: JoinHandle<()>,
    abort_tx: mpsc::Sender<()>,
}

/// Task events
#[derive(Debug)]
pub enum TaskEvent {
    Started(TaskId),
    Completed(TaskId, TaskResult),
    Failed(TaskId, String),
    Cancelled(TaskId),
}

impl TaskManager {
    /// Create a new task manager
    pub fn new() -> (Self, mpsc::UnboundedReceiver<TaskEvent>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        
        let manager = Self {
            next_id: AtomicU64::new(1),
            tasks: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
        };

        (manager, event_rx)
    }

    /// Submit a new task
    pub async fn submit(&self, task_type: TaskType) -> TaskId {
        let id = TaskId(self.next_id.fetch_add(1, Ordering::SeqCst));
        let task = BackgroundTask::new(id, task_type);

        {
            let mut tasks = self.tasks.write().await;
            tasks.insert(id, task);
        }

        debug!("Submitted background task {}", id);
        id
    }

    /// Start executing a task
    pub async fn start(&self, id: TaskId) -> anyhow::Result<()> {
        let task = {
            let tasks = self.tasks.read().await;
            tasks.get(&id).cloned().ok_or_else(|| anyhow::anyhow!("Task not found"))?
        };

        if task.status != TaskStatus::Pending {
            return Err(anyhow::anyhow!("Task is not pending"));
        }

        // Update status
        {
            let mut tasks = self.tasks.write().await;
            if let Some(t) = tasks.get_mut(&id) {
                t.status = TaskStatus::Running;
                t.started_at = Some(Utc::now());
            }
        }

        let (abort_tx, mut abort_rx) = mpsc::channel(1);
        let event_tx = self.event_tx.clone();
        let tasks = self.tasks.clone();

        let handle = match &task.task_type {
            TaskType::ShellCommand { command, working_dir } => {
                let command = command.clone();
                let working_dir = working_dir.clone();
                
                tokio::spawn(async move {
                    let result = run_shell_task(command, working_dir, &mut abort_rx).await;
                    handle_task_completion(id, result, &tasks, &event_tx).await;
                })
            }
            TaskType::AgentTask { prompt, context } => {
                let prompt = prompt.clone();
                let _context = context.clone();
                
                tokio::spawn(async move {
                    // Placeholder for agent task execution
                    let result = TaskResult {
                        success: true,
                        output: format!("Agent task completed: {}", prompt),
                        exit_code: Some(0),
                        error: None,
                    };
                    
                    tokio::select! {
                        _ = tokio::time::sleep(tokio::time::Duration::from_secs(5)) => {}
                        _ = abort_rx.recv() => {}
                    }
                    
                    handle_task_completion(id, Ok(result), &tasks, &event_tx).await;
                })
            }
            TaskType::FileWatch { path, pattern } => {
                let path = path.clone();
                let pattern = pattern.clone();
                
                tokio::spawn(async move {
                    let result = run_file_watch(path, pattern, &mut abort_rx).await;
                    handle_task_completion(id, result, &tasks, &event_tx).await;
                })
            }
            TaskType::HttpRequest { url, method, body } => {
                let url = url.clone();
                let method = method.clone();
                let body = body.clone();
                
                tokio::spawn(async move {
                    let result = run_http_request(url, method, body, &mut abort_rx).await;
                    handle_task_completion(id, result, &tasks, &event_tx).await;
                })
            }
        };

        {
            let mut running = self.running.write().await;
            running.insert(id, RunningTask { handle, abort_tx });
        }

        let _ = self.event_tx.send(TaskEvent::Started(id));
        info!("Started background task {}", id);

        Ok(())
    }

    /// Cancel a task
    pub async fn cancel(&self, id: TaskId) -> anyhow::Result<()> {
        let mut running = self.running.write().await;
        
        if let Some(task) = running.remove(&id) {
            let _ = task.abort_tx.send(()).await;
            task.handle.abort();
            
            let mut tasks = self.tasks.write().await;
            if let Some(t) = tasks.get_mut(&id) {
                t.status = TaskStatus::Cancelled;
                t.completed_at = Some(Utc::now());
            }
            
            let _ = self.event_tx.send(TaskEvent::Cancelled(id));
            info!("Cancelled background task {}", id);
        }

        Ok(())
    }

    /// Get task by ID
    pub async fn get(&self, id: TaskId) -> Option<BackgroundTask> {
        let tasks = self.tasks.read().await;
        tasks.get(&id).cloned()
    }

    /// List all tasks
    pub async fn list(&self) -> Vec<BackgroundTask> {
        let tasks = self.tasks.read().await;
        tasks.values().cloned().collect()
    }

    /// List tasks by status
    pub async fn list_by_status(&self, status: TaskStatus) -> Vec<BackgroundTask> {
        let tasks = self.tasks.read().await;
        tasks.values()
            .filter(|t| t.status == status)
            .cloned()
            .collect()
    }

    /// Get active task count
    pub async fn active_count(&self) -> usize {
        let running = self.running.read().await;
        running.len()
    }

    /// Clean up completed tasks
    pub async fn cleanup(&self, max_age: std::time::Duration) -> usize {
        let mut tasks = self.tasks.write().await;
        let now = Utc::now();
        
        let to_remove: Vec<_> = tasks
            .iter()
            .filter(|(_, t)| {
                !t.status.is_active() && 
                t.completed_at.map(|at| now - at > chrono::Duration::from_std(max_age).unwrap_or_default()).unwrap_or(false)
            })
            .map(|(id, _)| *id)
            .collect();
        
        let count = to_remove.len();
        for id in to_remove {
            tasks.remove(&id);
        }
        
        count
    }

    /// Format tasks table
    pub async fn format_table(&self) -> String {
        let tasks = self.list().await;
        
        if tasks.is_empty() {
            return "No background tasks.".to_string();
        }

        let mut lines = vec![
            "Background Tasks:".to_string(),
            "==================".to_string(),
        ];

        for task in tasks {
            lines.push(task.format_summary());
            if let Some(ref result) = task.result {
                if !result.output.is_empty() {
                    let preview = if result.output.len() > 50 {
                        format!("{}...", &result.output[..50])
                    } else {
                        result.output.clone()
                    };
                    lines.push(format!("  Output: {}", preview));
                }
            }
        }

        lines.join("\n")
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new().0
    }
}

/// Handle task completion
async fn handle_task_completion(
    id: TaskId,
    result: anyhow::Result<TaskResult>,
    tasks: &Arc<RwLock<HashMap<TaskId, BackgroundTask>>>,
    event_tx: &mpsc::UnboundedSender<TaskEvent>,
) {
    let mut task_list = tasks.write().await;
    
    if let Some(task) = task_list.get_mut(&id) {
        task.completed_at = Some(Utc::now());
        
        match result {
            Ok(res) => {
                task.status = if res.success {
                    TaskStatus::Completed
                } else {
                    TaskStatus::Failed
                };
                task.result = Some(res.clone());
                
                if res.success {
                    let _ = event_tx.send(TaskEvent::Completed(id, res));
                } else {
                    let _ = event_tx.send(TaskEvent::Failed(id, res.output));
                }
            }
            Err(e) => {
                task.status = TaskStatus::Failed;
                task.result = Some(TaskResult {
                    success: false,
                    output: e.to_string(),
                    exit_code: None,
                    error: Some(e.to_string()),
                });
                let _ = event_tx.send(TaskEvent::Failed(id, e.to_string()));
            }
        }
    }
}

/// Run shell task
async fn run_shell_task(
    command: String,
    working_dir: Option<String>,
    abort_rx: &mut mpsc::Receiver<()>,
) -> anyhow::Result<TaskResult> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(&command);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    
    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    let mut child = cmd.spawn()?;

    tokio::select! {
        status = child.wait() => {
            let status = status?;
            let mut stdout = String::new();
            let mut stderr = String::new();
            
            if let Some(mut out) = child.stdout.take() {
                use tokio::io::AsyncReadExt;
                out.read_to_string(&mut stdout).await?;
            }
            if let Some(mut err) = child.stderr.take() {
                use tokio::io::AsyncReadExt;
                err.read_to_string(&mut stderr).await?;
            }

            let output = if stdout.is_empty() { stderr } else { stdout };
            
            Ok(TaskResult {
                success: status.success(),
                output,
                exit_code: status.code(),
                error: None,
            })
        }
        _ = abort_rx.recv() => {
            child.kill().await?;
            Err(anyhow::anyhow!("Task cancelled"))
        }
    }
}

/// Run file watch task
async fn run_file_watch(
    _path: String,
    _pattern: Option<String>,
    abort_rx: &mut mpsc::Receiver<()>,
) -> anyhow::Result<TaskResult> {
    // Placeholder implementation
    tokio::select! {
        _ = tokio::time::sleep(tokio::time::Duration::from_secs(3600)) => {
            Ok(TaskResult {
                success: true,
                output: "File watch completed".to_string(),
                exit_code: Some(0),
                error: None,
            })
        }
        _ = abort_rx.recv() => {
            Err(anyhow::anyhow!("Task cancelled"))
        }
    }
}

/// Run HTTP request task
async fn run_http_request(
    _url: String,
    _method: String,
    _body: Option<String>,
    abort_rx: &mut mpsc::Receiver<()>,
) -> anyhow::Result<TaskResult> {
    // HTTP requests require reqwest, using placeholder for now
    tokio::select! {
        _ = tokio::time::sleep(tokio::time::Duration::from_secs(30)) => {
            Ok(TaskResult {
                success: true,
                output: "HTTP request completed (placeholder)".to_string(),
                exit_code: Some(0),
                error: None,
            })
        }
        _ = abort_rx.recv() => {
            Err(anyhow::anyhow!("Task cancelled"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_id_display() {
        let id = TaskId::new(42);
        assert_eq!(format!("{}", id), "#42");
    }

    #[test]
    fn test_task_status() {
        assert!(TaskStatus::Running.is_active());
        assert!(TaskStatus::Pending.is_active());
        assert!(!TaskStatus::Completed.is_active());
        assert!(!TaskStatus::Failed.is_active());
    }

    #[tokio::test]
    async fn test_task_manager_submit() {
        let (manager, _events) = TaskManager::new();
        
        let task_type = TaskType::ShellCommand {
            command: "echo hello".to_string(),
            working_dir: None,
        };
        
        let id = manager.submit(task_type.clone()).await;
        let task = manager.get(id).await.unwrap();
        
        assert_eq!(task.id, id);
        assert_eq!(task.task_type, task_type);
        assert_eq!(task.status, TaskStatus::Pending);
    }

    #[tokio::test]
    async fn test_task_manager_list() {
        let (manager, _events) = TaskManager::new();
        
        let tasks = manager.list().await;
        assert!(tasks.is_empty());
        
        manager.submit(TaskType::ShellCommand {
            command: "echo 1".to_string(),
            working_dir: None,
        }).await;
        
        manager.submit(TaskType::ShellCommand {
            command: "echo 2".to_string(),
            working_dir: None,
        }).await;
        
        let tasks = manager.list().await;
        assert_eq!(tasks.len(), 2);
    }
}
