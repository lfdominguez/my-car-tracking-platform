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
mod tools;

pub use agent::analyze_trip;
pub use chat::{
    run_chat, ChatEvent, ChatOptions, ChatSink, ChatToolbox, ChatTurnResult, NullSink,
    ToolInvocation, MAX_CHAT_TURNS,
};
pub use context::*;
pub use error::AiError;
pub use prompt::{chat_system_prompt, ChatCarBrief};
pub use report::*;
