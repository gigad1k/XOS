//! A provider for llama.cpp server, Ollama, or anything else exposing an
//! OpenAI-compatible `/chat/completions`.
//!
//! Prefix caching is the reason this file is more than a thin HTTP wrapper.
//! llama.cpp keeps a KV cache per slot; if the same slot sees the same prompt
//! prefix again it skips reprocessing it. Compiled prompts are stable per task
//! type, so pinning each `cache_key` to its own slot turns a repeated prefix
//! from a full prefill into almost nothing. That is the single largest latency
//! win available, so it is wired in from the start.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{Capabilities, CompletionRequest, Modality, Provider, ProviderError, TokenStream};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlamaCppConfig {
    pub base_url: String,
    pub model: String,
    #[serde(default = "default_context_window")]
    pub context_window: u32,
    #[serde(default = "default_true")]
    pub supports_tools: bool,
    #[serde(default = "default_true")]
    pub local: bool,
    #[serde(default)]
    pub cost_per_1k_input: f64,
    #[serde(default)]
    pub cost_per_1k_output: f64,
    /// How many llama.cpp slots the server was started with. Zero disables
    /// prefix caching, which is correct for backends that have no slots.
    #[serde(default)]
    pub slots: usize,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

fn default_context_window() -> u32 {
    8192
}

fn default_true() -> bool {
    true
}

fn default_timeout_secs() -> u64 {
    300
}

impl Default for LlamaCppConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:11434/v1".to_string(),
            model: "gemma4:e4b".to_string(),
            context_window: default_context_window(),
            supports_tools: true,
            local: true,
            cost_per_1k_input: 0.0,
            cost_per_1k_output: 0.0,
            slots: 0,
            timeout_secs: default_timeout_secs(),
            api_key: None,
        }
    }
}

pub struct LlamaCppProvider {
    name: String,
    config: LlamaCppConfig,
    client: reqwest::Client,
    /// cache_key to slot index. A key keeps its slot for the process lifetime so
    /// its prefix stays warm.
    slots: Mutex<HashMap<String, usize>>,
}

impl LlamaCppProvider {
    pub fn new(name: &str, config: LlamaCppConfig) -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| ProviderError::Transport(e.to_string()))?;
        Ok(Self {
            name: name.to_string(),
            config,
            client,
            slots: Mutex::new(HashMap::new()),
        })
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        )
    }

    /// Map a cache key to a stable slot, assigning the next free one.
    ///
    /// When every slot is taken the oldest assignment is reused: a cold prefix
    /// is slower, never wrong.
    fn slot_for(&self, cache_key: &str) -> Option<usize> {
        if self.config.slots == 0 {
            return None;
        }
        let mut slots = self.slots.lock().ok()?;
        if let Some(slot) = slots.get(cache_key) {
            return Some(*slot);
        }
        let next = slots.len() % self.config.slots;
        slots.insert(cache_key.to_string(), next);
        Some(next)
    }

    fn body(&self, request: &CompletionRequest) -> Value {
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
            // The router judges confidence from these. Endpoints that do not
            // support them ignore the field and the trigger simply never fires.
            "logprobs": true,
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
        if let Some(key) = &request.cache_key {
            if let Some(slot) = self.slot_for(key) {
                // llama.cpp reads these; backends without slots ignore them.
                body["cache_prompt"] = json!(true);
                body["id_slot"] = json!(slot);
            }
        }
        body
    }
}

#[async_trait]
impl Provider for LlamaCppProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            context_window: self.config.context_window,
            supports_tools: self.config.supports_tools,
            supports_prefix_cache: self.config.slots > 0,
            modalities: vec![Modality::Text],
            cost_per_1k_input: self.config.cost_per_1k_input,
            cost_per_1k_output: self.config.cost_per_1k_output,
            local: self.config.local,
        }
    }

    async fn complete(&self, request: CompletionRequest) -> Result<TokenStream, ProviderError> {
        let mut builder = self.client.post(self.endpoint()).json(&self.body(&request));
        if let Some(key) = &self.config.api_key {
            builder = builder.bearer_auth(key);
        }

        let response = builder
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let body: String = body.chars().take(400).collect();
            return Err(ProviderError::Http {
                status: status.as_u16(),
                body,
            });
        }

        Ok(super::sse::read(response, super::sse::openai_token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::Message;

    fn user(text: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: text.to_string(),
        }
    }

    fn provider(slots: usize) -> LlamaCppProvider {
        LlamaCppProvider::new(
            "local",
            LlamaCppConfig {
                slots,
                ..Default::default()
            },
        )
        .expect("provider builds")
    }

    #[test]
    fn a_cache_key_keeps_its_slot() {
        let provider = provider(4);
        let first = provider.slot_for("triage-v1");
        let again = provider.slot_for("triage-v1");
        assert_eq!(first, again, "the same key must stay on the same slot");
        assert_ne!(first, provider.slot_for("summarise-v1"));
    }

    #[test]
    fn slots_wrap_rather_than_overflow() {
        let provider = provider(2);
        let assigned: Vec<Option<usize>> = ["a", "b", "c", "d"]
            .iter()
            .map(|key| provider.slot_for(key))
            .collect();
        for slot in assigned.into_iter().flatten() {
            assert!(slot < 2, "slot {} is outside the configured range", slot);
        }
    }

    #[test]
    fn prefix_caching_is_off_without_slots() {
        let provider = provider(0);
        assert_eq!(provider.slot_for("triage-v1"), None);
        assert!(!provider.capabilities().supports_prefix_cache);
    }

    #[test]
    fn the_body_carries_slot_fields_only_when_keyed() {
        let provider = provider(4);
        let plain = provider.body(&CompletionRequest {
            messages: vec![user("hello")],
            ..Default::default()
        });
        assert!(plain.get("id_slot").is_none());
        assert!(plain.get("cache_prompt").is_none());

        let keyed = provider.body(&CompletionRequest {
            messages: vec![user("hello")],
            cache_key: Some("triage-v1".to_string()),
            ..Default::default()
        });
        assert_eq!(keyed["cache_prompt"], json!(true));
        assert!(keyed.get("id_slot").is_some());
    }

}
