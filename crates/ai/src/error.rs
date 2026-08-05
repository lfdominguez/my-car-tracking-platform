use thiserror::Error;

#[derive(Debug, Error)]
pub enum AiError {
    #[error("openrouter/agent error: {0}")]
    Agent(String),
    #[error("model did not submit a valid analysis report")]
    MissingReport,
    #[error("invalid report: {0}")]
    InvalidReport(String),
    #[error("tool error: {0}")]
    Tool(String),
}
