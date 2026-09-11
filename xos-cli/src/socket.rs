//! Finding and talking to the daemon socket.
//!
//! The daemon prefers `/run/xosd.sock` and falls back to the user's runtime
//! directory when it cannot write there. The client tries the same places in
//! the same order, so a developer running `xosd` as themselves needs no flags.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};

#[cfg(unix)]
use std::os::unix::net::UnixStream;

const SYSTEM_SOCKET: &str = "/run/xosd.sock";

#[derive(Debug, Deserialize)]
struct PartialConfig {
    socket: Option<PathBuf>,
}

/// Socket paths to try, most specific first.
fn candidates(explicit: Option<PathBuf>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = explicit {
        paths.push(path);
        return paths;
    }
    if let Some(path) = configured_socket() {
        paths.push(path);
    }
    paths.push(PathBuf::from(SYSTEM_SOCKET));
    if let Some(runtime) = dirs::runtime_dir() {
        paths.push(runtime.join("xosd.sock"));
    }
    paths.push(std::env::temp_dir().join("xosd.sock"));
    paths.dedup();
    paths
}

fn configured_socket() -> Option<PathBuf> {
    let path = dirs::config_dir()?.join("xos").join("config.toml");
    let text = std::fs::read_to_string(path).ok()?;
    let config: PartialConfig = toml::from_str(&text).ok()?;
    config.socket
}

pub struct Connection {
    #[cfg(unix)]
    stream: UnixStream,
    path: PathBuf,
    next_id: u64,
}

impl Connection {
    #[cfg(unix)]
    pub fn open(explicit: Option<PathBuf>) -> Result<Self, String> {
        let tried = candidates(explicit);
        for path in &tried {
            if let Ok(stream) = UnixStream::connect(path) {
                return Ok(Self {
                    stream,
                    path: path.clone(),
                    next_id: 1,
                });
            }
        }
        let list: Vec<String> = tried.iter().map(|p| p.display().to_string()).collect();
        Err(format!(
            "cannot reach the daemon. Tried: {}",
            list.join(", ")
        ))
    }

    #[cfg(not(unix))]
    pub fn open(_explicit: Option<PathBuf>) -> Result<Self, String> {
        Err("the XOS daemon socket needs a Unix system".to_string())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Send one request and read until its reply arrives, skipping the
    /// streaming notifications the daemon may interleave.
    #[cfg(unix)]
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;

        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut line = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        line.push(b'\n');
        self.stream
            .write_all(&line)
            .map_err(|e| format!("cannot send `{}`: {}", method, e))?;
        self.stream.flush().map_err(|e| e.to_string())?;

        let reader = BufReader::new(
            self.stream
                .try_clone()
                .map_err(|e| format!("cannot read the reply: {}", e))?,
        );
        for line in reader.lines() {
            let line = line.map_err(|e| format!("cannot read the reply: {}", e))?;
            if line.trim().is_empty() {
                continue;
            }
            let message: Value = match serde_json::from_str(&line) {
                Ok(value) => value,
                Err(_) => continue,
            };
            // Notifications carry no id; they belong to a streaming call.
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                let text = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("the daemon refused the call");
                return Err(text.to_string());
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(format!("the daemon closed the connection during `{}`", method))
    }

    #[cfg(not(unix))]
    pub fn call(&mut self, _method: &str, _params: Value) -> Result<Value, String> {
        Err("the XOS daemon socket needs a Unix system".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_socket_wins() {
        let chosen = candidates(Some(PathBuf::from("/tmp/explicit.sock")));
        assert_eq!(chosen, vec![PathBuf::from("/tmp/explicit.sock")]);
    }

    #[test]
    fn the_system_socket_is_tried() {
        let chosen = candidates(None);
        assert!(chosen.contains(&PathBuf::from(SYSTEM_SOCKET)));
        assert!(chosen.len() > 1, "there must be a fallback");
    }
}
