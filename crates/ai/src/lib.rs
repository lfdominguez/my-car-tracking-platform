//! Trip route analysis via OpenRouter (mechanic + financial coach).
//!
//! The server builds a [`TripAnalysisContext`] from DB data and calls [`analyze_trip`].
//! Tool schemas still use Rig's `Tool` trait; the multi-turn loop talks to OpenRouter
//! through a tolerant HTTP client (Rig's built-in OpenRouter parser is too strict).

mod agent;
mod context;
mod error;
mod math;
mod openrouter;
mod prompt;
mod report;
mod tools;

pub use agent::analyze_trip;
pub use context::*;
pub use error::AiError;
pub use report::*;
