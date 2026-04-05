//! Workflow engine for complex task execution
//!
//! Allows defining multi-step workflows that can be executed
//! with state tracking and conditional branching.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Workflow definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    pub id: String,
    pub name: String,
    pub description: String,
    pub steps: Vec<Step>,
    pub variables: HashMap<String, Variable>,
}

/// Workflow step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub action: StepAction,
    /// Next step to execute (None = end workflow)
    pub next: Option<String>,
    /// Condition to execute this step
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<Condition>,
    /// Error handling
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_error: Option<ErrorHandling>,
}

/// Step action types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StepAction {
    /// Execute a tool
    #[serde(rename = "tool")]
    Tool { name: String, params: serde_json::Value },
    /// LLM inference
    #[serde(rename = "llm")]
    Llm { prompt: String, model: Option<String> },
    /// Set variable
    #[serde(rename = "set")]
    Set { variable: String, value: serde_json::Value },
    /// User input
    #[serde(rename = "input")]
    Input { question: String, variable: String },
    /// Branching
    #[serde(rename = "branch")]
    Branch { branches: Vec<Branch> },
}

/// Conditional branch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    pub condition: Condition,
    pub target: String,
}

/// Condition for step execution
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Condition {
    /// Simple variable exists check
    Exists(String),
    /// Variable equals value
    Equals { var: String, value: serde_json::Value },
    /// Variable contains value
    Contains { var: String, value: String },
    /// Complex expression (evaluated by engine)
    Expression(String),
}

/// Error handling strategy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorHandling {
    pub action: ErrorAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorAction {
    Retry,
    Skip,
    Fail,
    Fallback,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub delay_ms: u64,
}

/// Workflow variable
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variable {
    #[serde(rename = "type")]
    pub var_type: VariableType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VariableType {
    String,
    Number,
    Boolean,
    Array,
    Object,
}

/// Workflow execution state
#[derive(Debug, Clone)]
pub struct WorkflowState {
    pub workflow_id: String,
    pub current_step: Option<String>,
    pub variables: HashMap<String, serde_json::Value>,
    pub step_results: HashMap<String, StepResult>,
    pub status: WorkflowStatus,
    pub error_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Step execution result
#[derive(Debug, Clone)]
pub struct StepResult {
    pub success: bool,
    pub output: serde_json::Value,
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// Workflow engine
pub struct WorkflowEngine {
    workflows: HashMap<String, Workflow>,
    active_states: HashMap<String, WorkflowState>,
}

impl Default for WorkflowEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkflowEngine {
    pub fn new() -> Self {
        Self {
            workflows: HashMap::new(),
            active_states: HashMap::new(),
        }
    }

    /// Register a workflow
    pub fn register(&mut self, workflow: Workflow) {
        self.workflows.insert(workflow.id.clone(), workflow);
    }

    /// Create a workflow from JSON
    pub fn register_from_json(&mut self, json: &str) -> Result<(), serde_json::Error> {
        let workflow: Workflow = serde_json::from_str(json)?;
        self.register(workflow);
        Ok(())
    }

    /// Start workflow execution
    pub fn start(&mut self, workflow_id: &str, initial_vars: HashMap<String, serde_json::Value>) -> Option<String> {
        let workflow = self.workflows.get(workflow_id)?;
        
        let execution_id = format!("{}-{}", workflow_id, uuid::Uuid::new_v4());
        
        let mut variables = initial_vars;
        // Set default values for variables
        for (name, var) in &workflow.variables {
            if !variables.contains_key(name) {
                if let Some(default) = &var.default {
                    variables.insert(name.clone(), default.clone());
                }
            }
        }

        let state = WorkflowState {
            workflow_id: workflow_id.to_string(),
            current_step: workflow.steps.first().map(|s| s.id.clone()),
            variables,
            step_results: HashMap::new(),
            status: WorkflowStatus::Running,
            error_count: 0,
        };

        self.active_states.insert(execution_id.clone(), state);
        Some(execution_id)
    }

    /// Get workflow state
    pub fn get_state(&self, execution_id: &str) -> Option<&WorkflowState> {
        self.active_states.get(execution_id)
    }

    /// Execute next step (simplified version)
    pub fn execute_next(&mut self, execution_id: &str) -> Option<StepResult> {
        // Get workflow and step info first
        let workflow_id = self.active_states.get(execution_id)?.workflow_id.clone();
        let workflow = self.workflows.get(&workflow_id)?;
        
        let current_step_id = self.active_states.get(execution_id)?.current_step.clone()?;
        let step = workflow.steps.iter().find(|s| s.id == current_step_id)?.clone();

        // Check condition
        if let Some(condition) = &step.condition {
            let vars = &self.active_states.get(execution_id)?.variables;
            if !self.evaluate_condition(condition, vars) {
                // Skip this step
                let state = self.active_states.get_mut(execution_id)?;
                state.current_step = step.next.clone();
                return Some(StepResult {
                    success: true,
                    output: serde_json::json!({"skipped": true}),
                    error: None,
                    duration_ms: 0,
                });
            }
        }

        // Execute action
        let start = std::time::Instant::now();
        let action_result = {
            let state = self.active_states.get_mut(execution_id)?;
            Self::execute_action_static(&step.action, state)
        };
        let duration = start.elapsed().as_millis() as u64;
        
        // Get mutable state for result handling
        let state = self.active_states.get_mut(execution_id)?;

        let step_result = match action_result {
            Ok(output) => {
                StepResult {
                    success: true,
                    output,
                    error: None,
                    duration_ms: duration,
                }
            }
            Err(e) => {
                state.error_count += 1;
                StepResult {
                    success: false,
                    output: serde_json::Value::Null,
                    error: Some(e),
                    duration_ms: duration,
                }
            }
        };

        state.step_results.insert(step.id.clone(), step_result.clone());

        // Determine next step
        if step_result.success {
            state.current_step = step.next.clone();
            if state.current_step.is_none() {
                state.status = WorkflowStatus::Completed;
            }
        } else if let Some(error_handling) = &step.on_error {
            match error_handling.action {
                ErrorAction::Retry => {
                    // Stay on current step for retry
                }
                ErrorAction::Skip => {
                    state.current_step = step.next.clone();
                }
                ErrorAction::Fail => {
                    state.status = WorkflowStatus::Failed;
                }
                ErrorAction::Fallback => {
                    state.current_step = error_handling.fallback.clone();
                }
            }
        } else {
            state.status = WorkflowStatus::Failed;
        }

        Some(step_result)
    }

    /// Evaluate condition
    fn evaluate_condition(&self, condition: &Condition, variables: &HashMap<String, serde_json::Value>) -> bool {
        match condition {
            Condition::Exists(var) => variables.contains_key(var),
            Condition::Equals { var, value } => {
                variables.get(var).map(|v| v == value).unwrap_or(false)
            }
            Condition::Contains { var, value } => {
                variables.get(var)
                    .and_then(|v| v.as_str())
                    .map(|v| v.contains(value))
                    .unwrap_or(false)
            }
            Condition::Expression(_) => {
                // Complex expressions not implemented in this simplified version
                true
            }
        }
    }

    /// Execute action (simplified, static version)
    fn execute_action_static(
        action: &StepAction,
        state: &mut WorkflowState,
    ) -> Result<serde_json::Value, String> {
        match action {
            StepAction::Set { variable, value } => {
                state.variables.insert(variable.clone(), value.clone());
                Ok(value.clone())
            }
            StepAction::Input { question, variable } => {
                // In real implementation, would prompt user
                // For now, just mark as waiting
                Ok(serde_json::json!({
                    "waiting_for_input": true,
                    "question": question,
                    "variable": variable
                }))
            }
            StepAction::Tool { name, params } => {
                // Would execute tool
                Ok(serde_json::json!({
                    "tool": name,
                    "params": params
                }))
            }
            StepAction::Llm { prompt, model } => {
                // Would call LLM
                Ok(serde_json::json!({
                    "prompt": prompt,
                    "model": model
                }))
            }
            StepAction::Branch { branches } => {
                // Would evaluate branches
                Ok(serde_json::json!({
                    "branches": branches.len()
                }))
            }
        }
    }

    /// Cancel workflow
    pub fn cancel(&mut self, execution_id: &str) -> bool {
        if let Some(state) = self.active_states.get_mut(execution_id) {
            state.status = WorkflowStatus::Cancelled;
            true
        } else {
            false
        }
    }

    /// Clean up completed workflows
    pub fn cleanup(&mut self) {
        self.active_states.retain(|_, state| {
            matches!(state.status, WorkflowStatus::Running | WorkflowStatus::Pending)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_workflow() -> Workflow {
        Workflow {
            id: "test-workflow".to_string(),
            name: "Test Workflow".to_string(),
            description: "A test workflow".to_string(),
            steps: vec![
                Step {
                    id: "step1".to_string(),
                    name: "Set Variable".to_string(),
                    action: StepAction::Set {
                        variable: "test_var".to_string(),
                        value: serde_json::json!("hello"),
                    },
                    next: Some("step2".to_string()),
                    condition: None,
                    on_error: None,
                },
                Step {
                    id: "step2".to_string(),
                    name: "Final Step".to_string(),
                    action: StepAction::Set {
                        variable: "done".to_string(),
                        value: serde_json::json!(true),
                    },
                    next: None,
                    condition: None,
                    on_error: None,
                },
            ],
            variables: HashMap::new(),
        }
    }

    #[test]
    fn test_workflow_engine() {
        let mut engine = WorkflowEngine::new();
        let workflow = create_test_workflow();
        
        engine.register(workflow);
        
        let exec_id = engine.start("test-workflow", HashMap::new());
        assert!(exec_id.is_some());
        
        let exec_id = exec_id.unwrap();
        
        // Execute first step
        let result1 = engine.execute_next(&exec_id);
        assert!(result1.is_some());
        assert!(result1.unwrap().success);
        
        // Execute second step
        let result2 = engine.execute_next(&exec_id);
        assert!(result2.is_some());
        assert!(result2.unwrap().success);
        
        // Check state
        let state = engine.get_state(&exec_id).unwrap();
        assert_eq!(state.status, WorkflowStatus::Completed);
    }

    #[test]
    fn test_condition_evaluation() {
        let engine = WorkflowEngine::new();
        let mut vars = HashMap::new();
        vars.insert("exists".to_string(), serde_json::json!("value"));
        vars.insert("number".to_string(), serde_json::json!(42));
        
        assert!(engine.evaluate_condition(&Condition::Exists("exists".to_string()), &vars));
        assert!(!engine.evaluate_condition(&Condition::Exists("missing".to_string()), &vars));
        
        assert!(engine.evaluate_condition(&Condition::Equals {
            var: "number".to_string(),
            value: serde_json::json!(42),
        }, &vars));
    }

    #[test]
    fn test_json_serialization() {
        let workflow = create_test_workflow();
        let json = serde_json::to_string(&workflow).unwrap();
        assert!(!json.is_empty());
        
        let deserialized: Workflow = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, workflow.id);
        assert_eq!(deserialized.steps.len(), workflow.steps.len());
    }
}
