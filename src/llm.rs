//! OpenCode Go LLM client — OpenAI-compatible chat completions.
//! Endpoint: https://opencode.ai/zen/go/v1/chat/completions

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: "system".into(), content: content.into() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: "user".into(), content: content.into() }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
}

#[derive(Debug, Clone, Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    format_type: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    content: Option<String>,
}

pub struct Llm {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: String,
}

impl Llm {
    pub fn new(model: String, api_key: String, base_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            model,
            api_key,
        }
    }

    /// Plain text completion.
    pub async fn complete(&self, messages: &[ChatMessage], temperature: f32) -> Result<String> {
        self.complete_inner(messages, temperature, false).await
    }

    /// Completion with JSON object output enforced.
    pub async fn complete_json(&self, messages: &[ChatMessage], temperature: f32) -> Result<String> {
        self.complete_inner(messages, temperature, true).await
    }

    async fn complete_inner(
        &self,
        messages: &[ChatMessage],
        temperature: f32,
        json_mode: bool,
    ) -> Result<String> {
        let req = ChatRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            temperature,
            response_format: if json_mode {
                Some(ResponseFormat { format_type: "json_object".into() })
            } else {
                None
            },
        };

        // Retry on transient errors (5xx, network) with backoff.
        let mut last_err: Option<anyhow::Error> = None;
        for attempt in 0..3 {
            if attempt > 0 {
                let delay = std::time::Duration::from_millis(1000 * 2u64.pow(attempt as u32));
                log_warn!("  ⚠ LLM retry {attempt}/3 in {}s...", delay.as_secs());
                tokio::time::sleep(delay).await;
            }
            match self
                .client
                .post(format!("{}/chat/completions", self.base_url))
                .bearer_auth(&self.api_key)
                .json(&req)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    let parsed: ChatResponse = match resp.json().await {
                        Ok(p) => p,
                        Err(e) => {
                            last_err = Some(e.into());
                            continue;
                        }
                    };
                    let content = parsed
                        .choices
                        .into_iter()
                        .next()
                        .and_then(|c| c.message.content)
                        .filter(|c| !c.trim().is_empty());
                    if let Some(c) = content {
                        return Ok(c);
                    }
                    last_err = Some(anyhow::anyhow!("LLM returned empty response"));
                }
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                        let body = resp.text().await.unwrap_or_default();
                        last_err = Some(anyhow::anyhow!("LLM API error {status}: {body}"));
                        continue; // transient → retry
                    }
                    let body = resp.text().await.unwrap_or_default();
                    bail!("LLM API error {status}: {body}");
                }
                Err(e) => {
                    last_err = Some(e.into());
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("LLM request failed after retries")))
    }
}
