//! Export and import: a portable, age-encrypted bundle.
//!
//! The whole value of XOS is that it remembers you, so a dead disk or a
//! reinstall must not erase that. The bundle is built alongside memory rather
//! than bolted on later, and it is encrypted with a passphrase so it can sit in
//! a backup or move between machines safely.
//!
//! It records **which providers were configured, never their secrets**. A
//! restored machine knows it should have an OpenRouter key and asks for one; it
//! does not carry the old one around inside a backup file.
//!
//! Import merges. Anything this machine already holds stays as it is, so
//! restoring an old bundle onto a live machine cannot roll it backwards.

use std::io::{Read, Write};
use std::path::Path;

use age::secrecy::SecretString;
use serde::{Deserialize, Serialize};

use super::Entry;

/// Bumped when the shape changes, so a future XOS can refuse a bundle it does
/// not understand rather than importing it wrongly.
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bundle {
    pub format_version: u32,
    pub created_unix: u64,
    pub memory: Vec<Entry>,
    /// The task graph, once P9 exists. Carried as opaque JSON so the bundle
    /// format does not have to change when the graph does.
    #[serde(default)]
    pub graph: serde_json::Value,
    /// Compiled prompts, once P8 exists.
    #[serde(default)]
    pub compiled_prompts: serde_json::Value,
    /// The config as it was, so a restore rebuilds the same setup.
    #[serde(default)]
    pub config: serde_json::Value,
    /// Provider names that had a credential. Names only. No key material is in
    /// this file, by construction.
    #[serde(default)]
    pub vault_providers: Vec<String>,
}

impl Bundle {
    pub fn new(memory: Vec<Entry>, config: serde_json::Value, vault_providers: Vec<String>) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            created_unix: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            memory,
            graph: serde_json::Value::Null,
            compiled_prompts: serde_json::Value::Null,
            config,
            vault_providers,
        }
    }
}

/// Write an encrypted bundle.
pub fn export(path: &Path, bundle: &Bundle, passphrase: &str) -> Result<(), String> {
    if passphrase.trim().is_empty() {
        return Err("a bundle needs a passphrase; it holds your memory".to_string());
    }
    let plaintext =
        serde_json::to_vec(bundle).map_err(|e| format!("cannot encode the bundle: {}", e))?;

    let encryptor = age::Encryptor::with_user_passphrase(SecretString::from(passphrase.to_string()));
    let mut ciphertext = Vec::new();
    let mut writer = encryptor
        .wrap_output(&mut ciphertext)
        .map_err(|e| format!("cannot seal the bundle: {}", e))?;
    writer
        .write_all(&plaintext)
        .map_err(|e| format!("cannot seal the bundle: {}", e))?;
    writer
        .finish()
        .map_err(|e| format!("cannot seal the bundle: {}", e))?;

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {}", parent.display(), e))?;
        }
    }
    std::fs::write(path, &ciphertext)
        .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Read an encrypted bundle.
pub fn import(path: &Path, passphrase: &str) -> Result<Bundle, String> {
    let ciphertext =
        std::fs::read(path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;

    let decryptor = age::Decryptor::new(&ciphertext[..])
        .map_err(|e| format!("this is not a bundle: {}", e))?;
    let identity = age::scrypt::Identity::new(SecretString::from(passphrase.to_string()));
    let mut reader = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .map_err(|_| "the passphrase does not open this bundle".to_string())?;

    let mut plaintext = Vec::new();
    reader
        .read_to_end(&mut plaintext)
        .map_err(|e| format!("cannot open the bundle: {}", e))?;

    let bundle: Bundle = serde_json::from_slice(&plaintext)
        .map_err(|e| format!("the bundle is malformed: {}", e))?;
    if bundle.format_version > FORMAT_VERSION {
        return Err(format!(
            "this bundle is version {}, and this XOS understands up to {}",
            bundle.format_version, FORMAT_VERSION
        ));
    }
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Memory, Tier};

    fn filled() -> (tempfile::TempDir, Memory) {
        let dir = tempfile::tempdir().expect("temp dir");
        let memory = Memory::in_memory().expect("memory");
        memory
            .write(Tier::LongTerm, "I prefer dark themes", "ui", "test")
            .expect("write");
        memory
            .write(Tier::Session, "fixed the backup job today", "backup", "test")
            .expect("write");
        (dir, memory)
    }

    #[test]
    fn a_bundle_round_trips() {
        let (dir, memory) = filled();
        let path = dir.path().join("xos.bundle");
        let bundle = Bundle::new(
            memory.all().expect("all"),
            serde_json::json!({"default_provider": "local"}),
            vec!["openrouter".to_string()],
        );

        export(&path, &bundle, "correct horse battery").expect("export");
        let restored = import(&path, "correct horse battery").expect("import");

        assert_eq!(restored.format_version, FORMAT_VERSION);
        assert_eq!(restored.memory.len(), 2);
        assert_eq!(restored.vault_providers, vec!["openrouter".to_string()]);
    }

    #[test]
    fn the_bundle_holds_provider_names_and_no_key_material() {
        let (dir, memory) = filled();
        let path = dir.path().join("xos.bundle");
        let bundle = Bundle::new(
            memory.all().expect("all"),
            serde_json::json!({}),
            vec!["openrouter".to_string(), "anthropic".to_string()],
        );
        export(&path, &bundle, "passphrase").expect("export");

        let restored = import(&path, "passphrase").expect("import");
        assert_eq!(restored.vault_providers.len(), 2);
        // The type carries no field a key could travel in.
        let json = serde_json::to_string(&restored).expect("encode");
        assert!(!json.contains("sk-"), "a key reached the bundle");
    }

    #[test]
    fn the_file_on_disk_is_encrypted() {
        let (dir, memory) = filled();
        let path = dir.path().join("xos.bundle");
        export(
            &path,
            &Bundle::new(memory.all().expect("all"), serde_json::json!({}), vec![]),
            "passphrase",
        )
        .expect("export");

        let bytes = std::fs::read(&path).expect("read");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("age-encryption.org/"), "not an age file");
        assert!(
            !text.contains("dark themes"),
            "memory is readable in the bundle"
        );
    }

    #[test]
    fn the_wrong_passphrase_is_refused_clearly() {
        let (dir, memory) = filled();
        let path = dir.path().join("xos.bundle");
        export(
            &path,
            &Bundle::new(memory.all().expect("all"), serde_json::json!({}), vec![]),
            "right",
        )
        .expect("export");

        let error = import(&path, "wrong").expect_err("a wrong passphrase must fail");
        assert!(error.contains("passphrase"), "{}", error);
    }

    #[test]
    fn an_empty_passphrase_is_refused() {
        let dir = tempfile::tempdir().expect("temp dir");
        let outcome = export(
            &dir.path().join("x.bundle"),
            &Bundle::new(vec![], serde_json::json!({}), vec![]),
            "   ",
        );
        assert!(outcome.is_err());
    }

    #[test]
    fn a_bundle_restores_into_an_empty_machine() {
        let (dir, memory) = filled();
        let path = dir.path().join("xos.bundle");
        export(
            &path,
            &Bundle::new(memory.all().expect("all"), serde_json::json!({}), vec![]),
            "pass",
        )
        .expect("export");

        let fresh = Memory::in_memory().expect("memory");
        let restored = import(&path, "pass").expect("import");
        let added = fresh.merge(&restored.memory).expect("merge");

        assert_eq!(added, 2, "a fresh machine takes everything");
        let hits = fresh.recall("dark themes", None, 5).expect("recall");
        assert!(!hits.is_empty(), "restored memory must be recallable");
    }

    #[cfg(unix)]
    #[test]
    fn the_bundle_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("xos.bundle");
        export(
            &path,
            &Bundle::new(vec![], serde_json::json!({}), vec![]),
            "pass",
        )
        .expect("export");
        let mode = std::fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
