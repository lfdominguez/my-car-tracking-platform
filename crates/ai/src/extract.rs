//! One-shot structured extraction: a single completion that must answer with a
//! JSON object, no tools and no multi-turn loop.

use serde::de::DeserializeOwned;
use serde_json::json;

use crate::agent::{extract_json_object, strip_code_fences};
use crate::error::AiError;
use crate::openrouter::OpenRouterClient;

/// Ask `model` for a JSON object and deserialize it into `T`. Markdown fences and
/// prose around the object are tolerated; anything else is [`AiError::Agent`].
pub async fn complete_json<T: DeserializeOwned>(
    api_key: &str,
    model: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<T, AiError> {
    let client = OpenRouterClient::new(api_key)?;
    complete_json_with(&client, model, system, user, max_tokens).await
}

async fn complete_json_with<T: DeserializeOwned>(
    client: &OpenRouterClient,
    model: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<T, AiError> {
    let messages = [
        json!({ "role": "system", "content": system }),
        json!({ "role": "user", "content": user }),
    ];
    let turn = client
        .chat_completion(model, &messages, &[], max_tokens)
        .await?;
    let text = turn
        .content
        .ok_or_else(|| AiError::Agent("model returned no content".into()))?;
    let body = strip_code_fences(&text);
    let candidate = extract_json_object(body).unwrap_or(body);
    serde_json::from_str(candidate).map_err(|e| AiError::Agent(format!("model JSON: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_http::{Scripted, serve};

    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct Out {
        quantity: Option<f64>,
    }

    fn reply(content: &str) -> String {
        json!({
            "model": "m",
            "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": content}}]
        })
        .to_string()
    }

    #[tokio::test]
    async fn parses_fenced_object_and_sends_no_tools() {
        let mock = serve(vec![Scripted::json(
            200,
            &reply("Here:\n```json\n{\"quantity\": 42.5}\n```"),
        )])
        .await;
        let client = OpenRouterClient::with_endpoint("sk", &mock.endpoint).unwrap();
        let out: Out = complete_json_with(&client, "m", "sys", "usr", 100)
            .await
            .unwrap();
        assert_eq!(
            out,
            Out {
                quantity: Some(42.5)
            }
        );
        let sent = mock.bodies().await;
        assert!(sent[0].get("tools").is_none());
        assert_eq!(sent[0]["messages"][1]["content"], "usr");
    }

    #[tokio::test]
    async fn non_json_answer_is_an_error() {
        let mock = serve(vec![Scripted::json(200, &reply("no idea"))]).await;
        let client = OpenRouterClient::with_endpoint("sk", &mock.endpoint).unwrap();
        let err = complete_json_with::<Out>(&client, "m", "sys", "usr", 100)
            .await
            .unwrap_err();
        assert!(matches!(err, AiError::Agent(_)), "{err:?}");
    }
}
