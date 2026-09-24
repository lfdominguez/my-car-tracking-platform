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
    // The provider-failure variants below keep a stable leading phrase: callers store
    // `to_string()` and later map it back to a user-facing line with
    // [`user_facing_error`], so the phrase is effectively part of the API.
    #[error("openrouter rejected the API key: {0}")]
    InvalidApiKey(String),
    #[error("openrouter account has insufficient credits: {0}")]
    InsufficientCredits(String),
    #[error("openrouter model not found: {0}")]
    ModelNotFound(String),
    #[error("openrouter rate limit exceeded: {0}")]
    RateLimited(String),
    #[error("openrouter timed out: {0}")]
    Timeout(String),
}

/// A short, caller-safe explanation for a stored AI job error, when the error is
/// one the user can act on. `None` means "show your generic failure line".
///
/// Works on the `to_string()` of an [`AiError`] (what job rows store), so it can be
/// applied long after the error value itself is gone.
pub fn user_facing_error(raw: &str) -> Option<&'static str> {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("api key is empty") {
        Some("Add your OpenRouter API key in Settings.")
    } else if lower.contains("openrouter rejected the api key") {
        Some("OpenRouter rejected your API key. Check it in Settings.")
    } else if lower.contains("openrouter account has insufficient credits") {
        Some("Your OpenRouter account is out of credits. Top it up, then try again.")
    } else if lower.contains("openrouter model not found") || lower.contains("model id is empty") {
        Some("The selected model is not available on OpenRouter. Pick another in Settings.")
    } else if lower.contains("openrouter rate limit exceeded") {
        Some("OpenRouter is rate limiting requests. Wait a minute and try again.")
    } else if lower.contains("openrouter timed out") {
        Some("OpenRouter took too long to answer. Try again in a moment.")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_errors_map_to_actionable_lines() {
        let cases = [
            AiError::InvalidApiKey("401".into()),
            AiError::InsufficientCredits("402".into()),
            AiError::ModelNotFound("x/y".into()),
            AiError::RateLimited("429".into()),
            AiError::Timeout("idle".into()),
        ];
        for (i, e) in cases.iter().enumerate() {
            assert!(user_facing_error(&e.to_string()).is_some(), "case {i}");
        }
        assert!(
            user_facing_error(&AiError::Agent("api key is empty".into()).to_string()).is_some()
        );
        assert_eq!(user_facing_error("sqlx: connection refused"), None);
    }
}
