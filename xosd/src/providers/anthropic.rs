//! The Anthropic messages API.
//!
//! Close enough to the OpenAI shape to share the SSE reader, different enough
//! to need its own event parsing: the system prompt is a top-level field rather
//! than a message, `max_tokens` is required, and usage arrives split across the
//! opening and closing events.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::sse;
use super::{
    Capabilities, CompletionRequest, Modality, Provider, ProviderError, Token, TokenStream, Usage,
};
use crate::vault::Vault;

const API_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicConfig {
    #[serde(default = "default_base_url")]
    pub base_url: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
    #[serde(default = "default_context_window")]
    pub context_window: u32,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default)]
    pub cost_per_1k_input: f64,
    #[serde(default)]
    pub cost_per_1k_output: f64,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_base_url() -> String {
    "https://api.anthropic.com/v1".to_string()
}

fn default_context_window() -> u32 {
    200_000
}

fn default_max_tokens() -> u32 {
    4096
}

fn default_timeout_secs() -> u64 {
    300
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            model: "claude-sonnet-5".to_string(),
            credential: None,
            context_window: default_context_window(),
            max_tokens: default_max_tokens(),
            cost_per_1k_input: 0.0,
            cost_per_1k_output: 0.0,
            timeout_secs: default_timeout_secs(),
        }
    }
}

pub struct AnthropicProvider {
    name: String,
    config: AnthropicConfig,
    client: reqwest::Client,
    vault: Arc<Vault>,
}

impl AnthropicProvider {
    pub fn new(
        name: &str,
        config: AnthropicConfig,
        vault: Arc<Vault>,
    ) -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| ProviderError::Transport(e.to_string()))?;
        Ok(Self {
            name: name.to_string(),
            config,
            client,
            vault,
        })
    }

    fn credential_name(&self) -> &str {
        self.config.credential.as_deref().unwrap_or(&self.name)
    }

    fn endpoint(&self) -> String {
        format!("{}/messages", self.config.base_url.trim_end_matches('/'))
    }

    /// Anthropic takes the system prompt beside the messages, not inside them.
    fn body(&self, request: &CompletionRequest) -> Value {
        let mut system = String::new();
        let mut messages: Vec<Value> = Vec::new();
        for message in &request.messages {
            if message.role == "system" {
                if !system.is_empty() {
                    system.push('\n');
                }
                system.push_str(&message.content);
            } else {
                messages.push(json!({"role": message.role, "content": message.content}));
            }
        }

        let mut body = json!({
            "model": self.config.model,
            "messages": messages,
            "max_tokens": request.max_tokens.unwrap_or(self.config.max_tokens),
            "stream": true,
        });
        if !system.is_empty() {
            body["system"] = Value::String(system);
        }
        if let Some(temperature) = request.temperature {
            body["temperature"] = json!(temperature);
        }
        if !request.tools.is_empty() {
            body["tools"] = Value::Array(request.tools.clone());
        }
        body
    }
}

/// Turn one Anthropic event into a token, or nothing when it carries none.
fn anthropic_token(event: &Value) -> Option<Token> {
    let kind = event.get("type").and_then(Value::as_str).unwrap_or_default();
    match kind {
        "content_block_delta" => {
            let text = event
                .pointer("/delta/text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if text.is_empty() {
                return None;
            }
            Some(Token {
                text: text.to_string(),
                finish_reason: None,
                usage: None,
            })
        }
        // Input tokens are reported when the message opens.
        "message_start" => {
            let usage = event.pointer("/message/usage")?;
            Some(Token {
                text: String::new(),
                finish_reason: None,
                usage: Some(Usage {
                    input_tokens: usage
                        .get("input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as u32,
                    output_tokens: 0,
                    cached_tokens: usage
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as u32,
                }),
            })
        }
        // Output tokens and the stop reason arrive at the end.
        "message_delta" => {
            let output = event
                .pointer("/usage/output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            let finish_reason = event
                .pointer("/delta/stop_reason")
                .and_then(Value::as_str)
                .map(str::to_string);
            if output == 0 && finish_reason.is_none() {
                return None;
            }
            Some(Token {
                text: String::new(),
                finish_reason,
                usage: Some(Usage {
                    input_tokens: 0,
                    output_tokens: output,
                    cached_tokens: 0,
                }),
            })
        }
        _ => None,
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            context_window: self.config.context_window,
            supports_tools: true,
            supports_prefix_cache: false,
            modalities: vec![Modality::Text, Modality::Vision],
            cost_per_1k_input: self.config.cost_per_1k_input,
            cost_per_1k_output: self.config.cost_per_1k_output,
            local: false,
        }
    }

    async fn complete(&self, request: CompletionRequest) -> Result<TokenStream, ProviderError> {
        let key = self
            .vault
            .get(self.credential_name())
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let response = self
            .client
            .post(self.endpoint())
            .header("x-api-key", key)
            .header("anthropic-version", API_VERSION)
            .json(&self.body(&request))
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::Http {
                status: status.as_u16(),
                body: body.chars().take(400).collect(),
            });
        }

        Ok(sse::read(response, anthropic_token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::Message;

    fn provider() -> AnthropicProvider {
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = Arc::new(Vault::with_file_backend(dir.path()));
        AnthropicProvider::new("anthropic", AnthropicConfig::default(), vault)
            .expect("provider builds")
    }

    fn message(role: &str, content: &str) -> Message {
        Message {
            role: role.to_string(),
            content: content.to_string(),
        }
    }

    #[test]
    fn the_system_prompt_is_lifted_out_of_the_messages() {
        let body = provider().body(&CompletionRequest {
            messages: vec![
                message("system", "Be terse."),
                message("user", "Hello"),
            ],
            ..Default::default()
        });
        assert_eq!(body["system"], json!("Be terse."));
        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 1, "the system turn must not be sent twice");
        assert_eq!(messages[0]["role"], json!("user"));
    }

    #[test]
    fn max_tokens_is_always_present_because_the_api_requires_it() {
        let body = provider().body(&CompletionRequest::default());
        assert!(body["max_tokens"].as_u64().unwrap_or(0) > 0);
    }

    #[test]
    fn a_text_delta_becomes_a_token() {
        let event = json!({"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}});
        assert_eq!(anthropic_token(&event).expect("token").text, "hi");
    }

    #[test]
    fn input_tokens_come_from_the_opening_event() {
        let event = json!({"type":"message_start","message":{"usage":{"input_tokens":42}}});
        let usage = anthropic_token(&event).expect("token").usage.expect("usage");
        assert_eq!(usage.input_tokens, 42);
        assert_eq!(usage.output_tokens, 0);
    }

    #[test]
    fn output_tokens_and_the_stop_reason_come_from_the_closing_event() {
        let event = json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},
                           "usage":{"output_tokens":17}});
        let token = anthropic_token(&event).expect("token");
        assert_eq!(token.finish_reason.as_deref(), Some("end_turn"));
        assert_eq!(token.usage.expect("usage").output_tokens, 17);
    }

    #[test]
    fn unrelated_events_are_skipped() {
        assert!(anthropic_token(&json!({"type":"ping"})).is_none());
        assert!(anthropic_token(&json!({"type":"content_block_start"})).is_none());
    }
}
