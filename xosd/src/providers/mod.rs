//! XOS Connect — connectors to model providers, MCP servers and channel gateways.
//!
//! The only place in XOS that knows a provider exists. Everything else reaches
//! inference, tools and channels through the daemon, never by importing a
//! provider directly.
//!
//! The trait below deliberately knows nothing about XOS. It has no notion of
//! routing, memory, policy or halting, so adding a provider stays a small job:
//! implement three methods and register it. Everything XOS-specific is layered
//! on top by the daemon.

pub mod llama_cpp;

use std::collections::BTreeMap;
use std::fmt;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures_core::Stream;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Modality {
    Text,
    Vision,
    Audio,
}

/// What a provider can do, so a caller can choose between them without
/// hardcoding names.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub context_window: u32,
    pub supports_tools: bool,
    /// The provider keeps a prompt prefix's KV cache warm between calls when
    /// given a `cache_key`. Load-bearing for latency, not an optimisation.
    pub supports_prefix_cache: bool,
    pub modalities: Vec<Modality>,
    pub cost_per_1k_input: f64,
    pub cost_per_1k_output: f64,
    /// True when inference runs on this machine, so it works offline.
    pub local: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<serde_json::Value>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Identifies a stable prompt prefix. A provider that supports prefix
    /// caching pins that prefix to a slot and skips reprocessing it on the next
    /// call carrying the same key. Compiled prompts depend on this.
    #[serde(default)]
    pub cache_key: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// Tokens served from a warm prefix rather than reprocessed, when the
    /// provider reports it.
    pub cached_tokens: u32,
}

/// One piece of a reply. The final item carries the finish reason and usage.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Token {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone)]
pub enum ProviderError {
    Transport(String),
    Http { status: u16, body: String },
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::Transport(detail) => write!(f, "transport: {}", detail),
            ProviderError::Http { status, body } => write!(f, "http {}: {}", status, body),
        }
    }
}

impl std::error::Error for ProviderError {}

pub type TokenStream = Pin<Box<dyn Stream<Item = Result<Token, ProviderError>> + Send>>;

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;

    fn capabilities(&self) -> Capabilities;

    /// Start a completion and return its token stream.
    async fn complete(&self, request: CompletionRequest) -> Result<TokenStream, ProviderError>;
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    pub name: String,
    pub capabilities: Capabilities,
}

/// Named providers, resolved by name at call time.
#[derive(Default, Clone)]
pub struct ProviderRegistry {
    providers: BTreeMap<String, Arc<dyn Provider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, provider: Arc<dyn Provider>) {
        self.providers.insert(provider.name().to_string(), provider);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Provider>> {
        self.providers.get(name).cloned()
    }

    pub fn list(&self) -> Vec<ProviderInfo> {
        self.providers
            .values()
            .map(|provider| ProviderInfo {
                name: provider.name().to_string(),
                capabilities: provider.capabilities(),
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Stub;

    #[async_trait]
    impl Provider for Stub {
        fn name(&self) -> &str {
            "stub"
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                context_window: 8192,
                supports_tools: true,
                supports_prefix_cache: false,
                modalities: vec![Modality::Text],
                cost_per_1k_input: 0.0,
                cost_per_1k_output: 0.0,
                local: true,
            }
        }

        async fn complete(&self, _request: CompletionRequest) -> Result<TokenStream, ProviderError> {
            Err(ProviderError::Transport("stub".to_string()))
        }
    }

    #[test]
    fn registry_resolves_by_name() {
        let mut registry = ProviderRegistry::new();
        registry.insert(Arc::new(Stub));
        assert!(registry.get("stub").is_some());
        assert!(registry.get("absent").is_none());
        assert_eq!(registry.list().len(), 1);
        assert_eq!(registry.list()[0].name, "stub");
    }

    #[test]
    fn a_request_carries_an_optional_cache_key() {
        let request: CompletionRequest =
            serde_json::from_str(r#"{"messages":[{"role":"user","content":"hello"}]}"#).unwrap();
        assert!(request.cache_key.is_none());

        let keyed: CompletionRequest = serde_json::from_str(
            r#"{"messages":[],"cache_key":"triage-v1"}"#,
        )
        .unwrap();
        assert_eq!(keyed.cache_key.as_deref(), Some("triage-v1"));
    }
}
