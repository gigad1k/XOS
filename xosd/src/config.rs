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

use crate::providers::llama_cpp::LlamaCppConfig;

pub const SYSTEM_SOCKET: &str = "/run/xosd.sock";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ProviderConfig {
    LlamaCpp(LlamaCppConfig),
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

/// Where the halt flag is persisted.
pub fn halt_state_path() -> PathBuf {
    dirs::state_dir()
        .or_else(|| dirs::data_local_dir())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("xos")
        .join("halted")
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
        }
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
