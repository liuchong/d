use std::future::Future;
use std::pin::Pin;

/// A plan is a sequence of steps to achieve a goal
#[derive(Debug, Clone)]
pub struct Plan {
    pub goal: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone)]
pub struct Step {
    pub description: String,
    pub tool: Option<String>,
    pub dependencies: Vec<usize>,
}

/// Planner trait for different planning strategies
pub trait Planner: Send + Sync {
    /// Create a plan to achieve the given goal
    fn plan<'a>(
        &'a self,
        goal: &'a str,
        context: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Plan>> + Send + 'a>>;
}

/// Simple LLM-based planner
#[allow(dead_code)]
pub struct LlmPlanner {
    client: llm::AiClient,
}

impl LlmPlanner {
    pub fn new(client: llm::AiClient) -> Self {
        Self { client }
    }
}

impl Planner for LlmPlanner {
    fn plan<'a>(
        &'a self,
        _goal: &'a str,
        _context: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Plan>> + Send + 'a>> {
        Box::pin(async move {
            // TODO: Implement planning prompt
            todo!("LLM planning")
        })
    }
}
