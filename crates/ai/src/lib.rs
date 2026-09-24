//! Trip route analysis via OpenRouter (mechanic + financial coach).
//!
//! The server builds a [`TripAnalysisContext`] from DB data and calls [`analyze_trip`].
//! Tool schemas still use Rig's `Tool` trait; the multi-turn loop talks to OpenRouter
//! through a tolerant HTTP client (Rig's built-in OpenRouter parser is too strict).

mod agent;
mod chat;
mod context;
mod error;
mod math;
mod openrouter;
mod prompt;
mod report;
#[cfg(test)]
mod test_http;
mod tools;

pub use agent::analyze_trip;
pub use chat::{
    ChatEvent, ChatOptions, ChatSink, ChatToolbox, ChatTurnResult, MAX_CHAT_TURNS, NullSink,
    TURN_CHAR_BUDGET, ToolInvocation, run_chat,
};
pub use context::*;
pub use error::{AiError, user_facing_error};
pub use prompt::{ChatCarBrief, chat_system_prompt, quoted_user_text, sanitize_user_text};
pub use report::*;

/// Model used when a user has not picked one. The single definition: the server's
/// fallbacks read this rather than repeating the id.
pub const DEFAULT_MODEL: &str = "anthropic/claude-3.7-sonnet";
