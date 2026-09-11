//! Configuration, written with defaults on first run.
//!
//! Lives at `~/.config/xos/config.toml`. The socket defaults to
//! `/run/xosd.sock`, which is where a packaged XOS puts it; when that directory
//! cannot be written — a developer running the daemon as themselves — the
//! daemon falls back to the user's runtime directory rather than refusing to
//! start.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::providers::anthropic::AnthropicConfig;
use crate::providers::llama_cpp::LlamaCppConfig;
use crate::memory::embed::EmbedderConfig;
use crate::providers::openai::OpenAiConfig;
use crate::router::RouterConfig;
use crate::spend::Caps;

pub const SYSTEM_SOCKET: &str = "/run/xosd.sock";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ProviderConfig {
    LlamaCpp(LlamaCppConfig),
    /// OpenRouter, OpenAI, Groq, DeepSeek, Together, or any compatible base URL.
    OpenAiCompatible(OpenAiConfig),
    Anthropic(AnthropicConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Where the daemon listens. A relative or unwritable path falls back to
    /// the runtime directory.
    #[serde(default = "default_socket")]
    pub socket: PathBuf,
    /// Which provider is used when a request names none.
    #[serde(default = "default_provider_name")]
    pub default_provider: String,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
    /// Daily spend ceilings. A provider over its cap becomes unavailable
    /// rather than failing partway through a reply.
    #[serde(default)]
    pub caps: Caps,
    /// Which tier serves what, and the thresholds that move a request.
    #[serde(default)]
    pub router: RouterConfig,
    /// How memory turns text into vectors for recall.
    #[serde(default)]
    pub embedder: EmbedderConfig,
    /// Scheduled export. Off unless asked for.
    #[serde(default)]
    pub export: ExportConfig,
}

/// A scheduled, encrypted export of everything XOS remembers.
///
/// Off by default, because it writes your memory to a path you chose and needs
/// a passphrase to do it. The passphrase is read from an environment variable
/// rather than stored in the config, so the config stays safe to copy around.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default = "default_export_hours")]
    pub every_hours: u64,
    /// Environment variable holding the passphrase.
    #[serde(default = "default_passphrase_env")]
    pub passphrase_env: String,
}

fn default_export_hours() -> u64 {
    24
}

fn default_passphrase_env() -> String {
    "XOS_EXPORT_PASSPHRASE".to_string()
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: None,
            every_hours: default_export_hours(),
            passphrase_env: default_passphrase_env(),
        }
    }
}

fn default_socket() -> PathBuf {
    PathBuf::from(SYSTEM_SOCKET)
}

fn default_provider_name() -> String {
    "local".to_string()
}

impl Default for Config {
    fn default() -> Self {
        let mut providers = BTreeMap::new();
        providers.insert(
            "local".to_string(),
            ProviderConfig::LlamaCpp(LlamaCppConfig::default()),
        );
        Self {
            socket: default_socket(),
            default_provider: default_provider_name(),
            providers,
            caps: Caps::default(),
            router: RouterConfig::default(),
            embedder: EmbedderConfig::default(),
            export: ExportConfig::default(),
        }
    }
}

/// `~/.config/xos/config.toml`
pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("xos")
        .join("config.toml")
}

/// Where the vault keeps its encrypted file and name index.
pub fn vault_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("xos")
}

/// Where memory lives.
pub fn memory_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("xos")
        .join("memory.db")
}

/// The escalation log database.
pub fn escalations_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("xos")
        .join("escalations.db")
}

/// Where a runtime cost-mode override is remembered.
///
/// The config file gives the default. `xos mode` writes here instead of
/// rewriting the config, so a runtime change never reformats a file a person
/// hand-edited or drops their comments.
pub fn mode_state_path() -> PathBuf {
    dirs::state_dir()
        .or_else(|| dirs::data_local_dir())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("xos")
        .join("mode")
}

/// The spend database.
pub fn spend_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("xos")
        .join("spend.db")
}

/// Where the halt flag is persisted.
pub fn halt_state_path() -> PathBuf {
    dirs::state_dir()
        .or_else(|| dirs::data_local_dir())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("xos")
        .join("halted")
}

/// The cost mode in force: the runtime override if one was set, else the
/// config's own value.
pub fn effective_mode(configured: RouterConfig) -> crate::router::CostMode {
    let path = mode_state_path();
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| crate::router::CostMode::parse(&text))
        .unwrap_or(configured.cost_mode)
}

/// Remember a runtime cost mode, without touching the config file.
pub fn write_mode(mode: crate::router::CostMode) -> io::Result<()> {
    let path = mode_state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{}\n", mode.label()))
}

impl Config {
    /// Read the config, writing defaults the first time.
    pub fn load_or_create(path: &Path) -> io::Result<Self> {
        if !path.exists() {
            let config = Config::default();
            config.write(path)?;
            return Ok(config);
        }
        let text = fs::read_to_string(path)?;
        toml::from_str(&text).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {}", path.display(), e),
            )
        })
    }

    pub fn write(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        fs::write(path, text)
    }

    /// The socket to bind, after checking the configured path is usable.
    ///
    /// Returns the path and, when it differs from the configured one, the
    /// reason so the caller can say so out loud rather than silently moving.
    pub fn resolve_socket(&self) -> (PathBuf, Option<String>) {
        let configured = self.socket.clone();
        if let Some(parent) = configured.parent() {
            if parent.as_os_str().is_empty() || writable(parent) {
                return (configured, None);
            }
            let fallback = runtime_socket();
            let reason = format!(
                "{} is not writable, listening on {} instead",
                parent.display(),
                fallback.display()
            );
            return (fallback, Some(reason));
        }
        (configured, None)
    }
}

fn writable(directory: &Path) -> bool {
    if !directory.exists() {
        return fs::create_dir_all(directory).is_ok();
    }
    // The cheapest honest test is to try, then clean up.
    let probe = directory.join(".xosd-write-probe");
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn runtime_socket() -> PathBuf {
    dirs::runtime_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("xosd.sock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_written_on_first_run() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("xos").join("config.toml");
        assert!(!path.exists());

        let config = Config::load_or_create(&path).expect("config is created");
        assert!(path.exists(), "first run must write the config");
        assert_eq!(config.default_provider, "local");
        assert!(config.providers.contains_key("local"));
    }

    #[test]
    fn an_existing_config_is_read_not_overwritten() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "socket = \"/tmp/custom.sock\"\ndefault_provider = \"cloud\"\n",
        )
        .expect("write config");

        let config = Config::load_or_create(&path).expect("config loads");
        assert_eq!(config.default_provider, "cloud");
        assert_eq!(config.socket, PathBuf::from("/tmp/custom.sock"));
    }

    #[test]
    fn config_round_trips_through_toml() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        Config::default().write(&path).expect("write");
        let reloaded = Config::load_or_create(&path).expect("reload");
        assert_eq!(reloaded.providers.len(), 1);
        match reloaded.providers.get("local").expect("local provider") {
            ProviderConfig::LlamaCpp(llama) => {
                assert!(llama.local);
                assert!(!llama.base_url.is_empty());
            }
            other => panic!("the default provider should be local, got {:?}", other),
        }
    }

    #[test]
    fn cloud_providers_and_caps_parse_from_toml() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"
default_provider = "local"

[caps]
daily_total = 5.0

[caps.daily_per_provider]
openrouter = 2.0

[providers.openrouter]
kind = "open-ai-compatible"
base_url = "https://openrouter.ai/api/v1"
model = "openai/gpt-4o-mini"
cost_per_1k_input = 0.00015

[providers.anthropic]
kind = "anthropic"
model = "claude-sonnet-5"
"#,
        )
        .expect("write config");

        let config = Config::load_or_create(&path).expect("config loads");
        assert_eq!(config.caps.daily_total, Some(5.0));
        assert_eq!(config.caps.daily_per_provider.get("openrouter"), Some(&2.0));
        assert!(matches!(
            config.providers.get("openrouter"),
            Some(ProviderConfig::OpenAiCompatible(_))
        ));
        assert!(matches!(
            config.providers.get("anthropic"),
            Some(ProviderConfig::Anthropic(_))
        ));
    }

    #[test]
    fn an_unwritable_socket_directory_falls_back_and_says_so() {
        let config = Config {
            socket: PathBuf::from("/proc/xos-not-writable/xosd.sock"),
            ..Default::default()
        };
        let (path, reason) = config.resolve_socket();
        assert!(reason.is_some(), "a move must be reported, not silent");
        assert_ne!(path, config.socket);
    }

    #[test]
    fn a_writable_socket_directory_is_kept() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = Config {
            socket: dir.path().join("xosd.sock"),
            ..Default::default()
        };
        let (path, reason) = config.resolve_socket();
        assert_eq!(path, config.socket);
        assert!(reason.is_none());
    }
}
