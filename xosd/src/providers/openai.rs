//! Any endpoint speaking the OpenAI chat API.
//!
//! One provider covers OpenRouter, OpenAI, Groq, DeepSeek, Together and
//! anything else with a compatible base URL, because they differ only in the
//! URL, the model name and the price. The key comes from the vault at call
//! time and is never held anywhere it could be logged.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::sse;
use super::{
    Capabilities, CompletionRequest, Modality, Provider, ProviderError, TokenStream,
};
use crate::vault::Vault;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiConfig {
    pub base_url: String,
    pub model: String,
    /// Which vault entry holds the key. Defaults to the provider's own name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
    #[serde(default = "default_context_window")]
    pub context_window: u32,
    #[serde(default = "default_true")]
    pub supports_tools: bool,
    #[serde(default)]
    pub cost_per_1k_input: f64,
    #[serde(default)]
    pub cost_per_1k_output: f64,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_context_window() -> u32 {
    128_000
}

fn default_true() -> bool {
    true
}

fn default_timeout_secs() -> u64 {
    300
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://openrouter.ai/api/v1".to_string(),
            model: "openai/gpt-4o-mini".to_string(),
            credential: None,
            context_window: default_context_window(),
            supports_tools: true,
            cost_per_1k_input: 0.0,
            cost_per_1k_output: 0.0,
            timeout_secs: default_timeout_secs(),
        }
    }
}

pub struct OpenAiCompatibleProvider {
    name: String,
    config: OpenAiConfig,
    client: reqwest::Client,
    vault: Arc<Vault>,
}

impl OpenAiCompatibleProvider {
    pub fn new(name: &str, config: OpenAiConfig, vault: Arc<Vault>) -> Result<Self, ProviderError> {
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
        format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        )
    }
}

#[async_trait]
impl Provider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            context_window: self.config.context_window,
            supports_tools: self.config.supports_tools,
            supports_prefix_cache: false,
            modalities: vec![Modality::Text],
            cost_per_1k_input: self.config.cost_per_1k_input,
            cost_per_1k_output: self.config.cost_per_1k_output,
            local: false,
        }
    }

    async fn complete(&self, request: CompletionRequest) -> Result<TokenStream, ProviderError> {
        // Fetched per call and dropped with the request, never cached in the
        // struct where a Debug print could reach it.
        let key = self
            .vault
            .get(self.credential_name())
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|m| json!({"role": m.role, "content": m.content}))
            .collect();
        let mut body = json!({
            "model": self.config.model,
            "messages": messages,
            "stream": true,
            "stream_options": {"include_usage": true},
        });
        if let Some(temperature) = request.temperature {
            body["temperature"] = json!(temperature);
        }
        if let Some(max_tokens) = request.max_tokens {
            body["max_tokens"] = json!(max_tokens);
        }
        if !request.tools.is_empty() {
            body["tools"] = Value::Array(request.tools.clone());
            body["tool_choice"] = json!("auto");
        }

        let response = self
            .client
            .post(self.endpoint())
            .bearer_auth(key)
            .json(&body)
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

        Ok(sse::read(response, sse::openai_token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(name: &str, credential: Option<&str>) -> OpenAiCompatibleProvider {
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = Arc::new(Vault::with_file_backend(dir.path()));
        OpenAiCompatibleProvider::new(
            name,
            OpenAiConfig {
                credential: credential.map(str::to_string),
                ..Default::default()
            },
            vault,
        )
        .expect("provider builds")
    }

    #[test]
    fn the_credential_defaults_to_the_provider_name() {
        assert_eq!(provider("openrouter", None).credential_name(), "openrouter");
    }

    #[test]
    fn a_credential_can_be_shared_between_providers() {
        assert_eq!(
            provider("groq-fast", Some("groq")).credential_name(),
            "groq"
        );
    }

    #[test]
    fn the_endpoint_tolerates_a_trailing_slash() {
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = Arc::new(Vault::with_file_backend(dir.path()));
        let provider = OpenAiCompatibleProvider::new(
            "openai",
            OpenAiConfig {
                base_url: "https://api.openai.com/v1/".to_string(),
                ..Default::default()
            },
            vault,
        )
        .expect("provider builds");
        assert_eq!(
            provider.endpoint(),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn a_cloud_provider_is_never_local() {
        assert!(!provider("openrouter", None).capabilities().local);
    }

    #[tokio::test]
    async fn a_missing_key_fails_without_naming_key_material() {
        let provider = provider("openrouter", None);
        let error = provider
            .complete(CompletionRequest::default())
            .await
            .err()
            .expect("no key means no call");
        let message = error.to_string();
        assert!(message.contains("openrouter"), "{}", message);
        assert!(!message.contains("sk-"), "{}", message);
    }
}
