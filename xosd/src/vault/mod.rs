//! XOS Vault — credentials held in the system keyring.
//!
//! Brokers secrets to subsystems that need them. Credentials are never written
//! to the repository, the journal or any API-bound context.
//!
//! # Where keys live
//!
//! The system keyring first. On a desktop that means the secret service, and the
//! key never touches a file XOS owns.
//!
//! When there is no keyring — a headless box, a container, a WSL session — the
//! fallback is an age-encrypted file. Be clear about what that buys: the
//! identity that decrypts it sits beside it on the same disk, both at `0600`, so
//! the protection is filesystem permissions plus encryption at rest, not
//! protection from someone who already reads your files as you. It stops a key
//! leaking through a backup, a synced folder or a stray `cat`. It is not a
//! substitute for a keyring, and the daemon says which one is in use on startup.
//!
//! Nothing here ever logs key material, puts it in an error, or hands it back
//! over the socket. `list` returns names.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use age::secrecy::SecretString;
use age::x25519;
use tracing::{debug, warn};

const SERVICE: &str = "xos";

#[derive(Debug)]
pub enum VaultError {
    /// Never carries key material, only what went wrong.
    Backend(String),
    NotFound(String),
}

impl std::fmt::Display for VaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VaultError::Backend(detail) => write!(f, "{}", detail),
            VaultError::NotFound(provider) => {
                write!(f, "no credential stored for `{}`", provider)
            }
        }
    }
}

impl std::error::Error for VaultError {}

/// Which store the vault settled on, so the daemon can say so out loud.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Keyring,
    EncryptedFile,
}

impl Backend {
    pub fn label(&self) -> &'static str {
        match self {
            Backend::Keyring => "system keyring",
            Backend::EncryptedFile => "age-encrypted file",
        }
    }
}

pub struct Vault {
    backend: Backend,
    /// Ciphertext, used only by the file backend.
    file: PathBuf,
    /// The age identity that opens the file, at 0600.
    identity_file: PathBuf,
}

impl Vault {
    /// Prefer the keyring; fall back to a file when none answers.
    pub fn open(directory: &Path) -> Self {
        let backend = if keyring_works() {
            Backend::Keyring
        } else {
            warn!(
                "no system keyring answered, falling back to an age-encrypted file. \
                 Its identity sits beside it at 0600, so the protection is file \
                 permissions plus encryption at rest."
            );
            Backend::EncryptedFile
        };
        Self {
            backend,
            file: directory.join("vault.age"),
            identity_file: directory.join("vault-identity.txt"),
        }
    }

    /// Force the file backend, bypassing the keyring probe.
    #[cfg(test)]
    pub fn with_file_backend(directory: &Path) -> Self {
        Self {
            backend: Backend::EncryptedFile,
            file: directory.join("vault.age"),
            identity_file: directory.join("vault-identity.txt"),
        }
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    pub fn set(&self, provider: &str, key: &str) -> Result<(), VaultError> {
        match self.backend {
            Backend::Keyring => {
                let entry = keyring::Entry::new(SERVICE, provider)
                    .map_err(|e| VaultError::Backend(format!("keyring: {}", e)))?;
                entry
                    .set_password(key)
                    .map_err(|e| VaultError::Backend(format!("keyring: {}", e)))
            }
            Backend::EncryptedFile => {
                let mut all = self.read_file()?;
                all.insert(provider.to_string(), key.to_string());
                self.write_file(&all)
            }
        }
    }

    pub fn get(&self, provider: &str) -> Result<String, VaultError> {
        match self.backend {
            Backend::Keyring => {
                let entry = keyring::Entry::new(SERVICE, provider)
                    .map_err(|e| VaultError::Backend(format!("keyring: {}", e)))?;
                match entry.get_password() {
                    Ok(key) => Ok(key),
                    Err(keyring::Error::NoEntry) => {
                        Err(VaultError::NotFound(provider.to_string()))
                    }
                    Err(error) => Err(VaultError::Backend(format!("keyring: {}", error))),
                }
            }
            Backend::EncryptedFile => self
                .read_file()?
                .get(provider)
                .cloned()
                .ok_or_else(|| VaultError::NotFound(provider.to_string())),
        }
    }

    pub fn remove(&self, provider: &str) -> Result<(), VaultError> {
        match self.backend {
            Backend::Keyring => {
                let entry = keyring::Entry::new(SERVICE, provider)
                    .map_err(|e| VaultError::Backend(format!("keyring: {}", e)))?;
                match entry.delete_credential() {
                    Ok(()) => Ok(()),
                    Err(keyring::Error::NoEntry) => {
                        Err(VaultError::NotFound(provider.to_string()))
                    }
                    Err(error) => Err(VaultError::Backend(format!("keyring: {}", error))),
                }
            }
            Backend::EncryptedFile => {
                let mut all = self.read_file()?;
                if all.remove(provider).is_none() {
                    return Err(VaultError::NotFound(provider.to_string()));
                }
                self.write_file(&all)
            }
        }
    }

    /// Provider names only. Never key material.
    ///
    /// A keyring cannot be enumerated portably, so the names of stored
    /// credentials are kept in a plain index file next to the vault. It holds
    /// names and nothing else.
    pub fn list(&self) -> Result<Vec<String>, VaultError> {
        match self.backend {
            Backend::Keyring => {
                let index = self.index_file();
                if !index.exists() {
                    return Ok(Vec::new());
                }
                let text = fs::read_to_string(&index)
                    .map_err(|e| VaultError::Backend(format!("cannot read the index: {}", e)))?;
                Ok(text
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_string)
                    .collect())
            }
            Backend::EncryptedFile => {
                let mut names: Vec<String> = self.read_file()?.keys().cloned().collect();
                names.sort();
                Ok(names)
            }
        }
    }

    /// Record a provider name in the keyring index, so `list` can report it.
    pub fn note_name(&self, provider: &str) -> Result<(), VaultError> {
        if self.backend != Backend::Keyring {
            return Ok(());
        }
        let mut names = self.list()?;
        if names.iter().any(|name| name == provider) {
            return Ok(());
        }
        names.push(provider.to_string());
        names.sort();
        let index = self.index_file();
        if let Some(parent) = index.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| VaultError::Backend(format!("cannot create the index: {}", e)))?;
        }
        fs::write(&index, names.join("\n") + "\n")
            .map_err(|e| VaultError::Backend(format!("cannot write the index: {}", e)))
    }

    pub fn forget_name(&self, provider: &str) -> Result<(), VaultError> {
        if self.backend != Backend::Keyring {
            return Ok(());
        }
        let names: Vec<String> = self
            .list()?
            .into_iter()
            .filter(|name| name != provider)
            .collect();
        fs::write(self.index_file(), names.join("\n") + "\n")
            .map_err(|e| VaultError::Backend(format!("cannot write the index: {}", e)))
    }

    fn index_file(&self) -> PathBuf {
        self.file.with_file_name("vault-index.txt")
    }

    // -- the encrypted file backend --------------------------------------

    fn identity(&self) -> Result<x25519::Identity, VaultError> {
        if self.identity_file.exists() {
            let text = fs::read_to_string(&self.identity_file)
                .map_err(|e| VaultError::Backend(format!("cannot read the identity: {}", e)))?;
            return text
                .trim()
                .parse::<x25519::Identity>()
                .map_err(|e| VaultError::Backend(format!("the identity is unreadable: {}", e)));
        }

        let identity = x25519::Identity::generate();
        if let Some(parent) = self.identity_file.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| VaultError::Backend(format!("cannot create the vault: {}", e)))?;
        }
        let secret: SecretString = identity.to_string();
        write_private(&self.identity_file, age::secrecy::ExposeSecret::expose_secret(&secret).as_bytes())?;
        Ok(identity)
    }

    fn read_file(&self) -> Result<BTreeMap<String, String>, VaultError> {
        if !self.file.exists() {
            return Ok(BTreeMap::new());
        }
        let identity = self.identity()?;
        let ciphertext = fs::read(&self.file)
            .map_err(|e| VaultError::Backend(format!("cannot read the vault: {}", e)))?;

        let decryptor = age::Decryptor::new(&ciphertext[..])
            .map_err(|e| VaultError::Backend(format!("the vault is unreadable: {}", e)))?;
        let mut reader = decryptor
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .map_err(|e| VaultError::Backend(format!("cannot open the vault: {}", e)))?;
        let mut plaintext = Vec::new();
        reader
            .read_to_end(&mut plaintext)
            .map_err(|e| VaultError::Backend(format!("cannot open the vault: {}", e)))?;

        serde_json::from_slice(&plaintext)
            .map_err(|e| VaultError::Backend(format!("the vault is malformed: {}", e)))
    }

    fn write_file(&self, all: &BTreeMap<String, String>) -> Result<(), VaultError> {
        let identity = self.identity()?;
        let plaintext = serde_json::to_vec(all)
            .map_err(|e| VaultError::Backend(format!("cannot encode the vault: {}", e)))?;

        let recipient = identity.to_public();
        let encryptor = age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
            .map_err(|e| VaultError::Backend(format!("cannot seal the vault: {}", e)))?;
        let mut ciphertext = Vec::new();
        let mut writer = encryptor
            .wrap_output(&mut ciphertext)
            .map_err(|e| VaultError::Backend(format!("cannot seal the vault: {}", e)))?;
        writer
            .write_all(&plaintext)
            .map_err(|e| VaultError::Backend(format!("cannot seal the vault: {}", e)))?;
        writer
            .finish()
            .map_err(|e| VaultError::Backend(format!("cannot seal the vault: {}", e)))?;

        write_private(&self.file, &ciphertext)
    }
}

/// Write a file only its owner can read.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), VaultError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| VaultError::Backend(format!("cannot create the vault: {}", e)))?;
    }
    fs::write(path, bytes)
        .map_err(|e| VaultError::Backend(format!("cannot write the vault: {}", e)))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|e| VaultError::Backend(format!("cannot secure the vault: {}", e)))?;
    }
    Ok(())
}

/// Probe the keyring without storing anything durable.
fn keyring_works() -> bool {
    let probe = match keyring::Entry::new(SERVICE, "xos-keyring-probe") {
        Ok(entry) => entry,
        Err(error) => {
            debug!(%error, "no keyring entry could be built");
            return false;
        }
    };
    match probe.set_password("probe") {
        Ok(()) => {
            let _ = probe.delete_credential();
            true
        }
        Err(error) => {
            debug!(%error, "the keyring refused a write");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = Vault::with_file_backend(dir.path());
        (dir, vault)
    }

    #[test]
    fn a_key_round_trips() {
        let (_dir, vault) = vault();
        vault.set("openrouter", "sk-test-value").expect("set");
        assert_eq!(vault.get("openrouter").expect("get"), "sk-test-value");
    }

    #[test]
    fn listing_returns_names_and_never_key_material() {
        let (_dir, vault) = vault();
        vault.set("openrouter", "sk-secret-one").expect("set");
        vault.set("anthropic", "sk-secret-two").expect("set");

        let names = vault.list().expect("list");
        assert_eq!(names, vec!["anthropic".to_string(), "openrouter".to_string()]);
        for name in &names {
            assert!(!name.contains("sk-"), "a key leaked into list()");
        }
    }

    #[test]
    fn a_missing_key_is_not_found_rather_than_an_error() {
        let (_dir, vault) = vault();
        match vault.get("absent") {
            Err(VaultError::NotFound(provider)) => assert_eq!(provider, "absent"),
            other => panic!("expected NotFound, got {:?}", other.map(|_| "a key")),
        }
    }

    #[test]
    fn the_file_on_disk_does_not_contain_the_key() {
        let (dir, vault) = vault();
        vault.set("openrouter", "sk-plaintext-canary").expect("set");

        let bytes = fs::read(dir.path().join("vault.age")).expect("read ciphertext");
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains("sk-plaintext-canary"),
            "the key is on disk in the clear"
        );
        assert!(text.starts_with("age-encryption.org/"), "not an age file");
    }

    #[test]
    fn an_error_never_carries_the_key() {
        let (_dir, vault) = vault();
        let message = vault.get("absent").unwrap_err().to_string();
        assert!(!message.contains("sk-"));
    }

    #[test]
    fn removing_a_key_takes_it_out_of_the_list() {
        let (_dir, vault) = vault();
        vault.set("groq", "sk-value").expect("set");
        vault.remove("groq").expect("remove");
        assert!(vault.list().expect("list").is_empty());
        assert!(vault.get("groq").is_err());
    }

    #[test]
    fn a_second_vault_reads_what_the_first_wrote() {
        let dir = tempfile::tempdir().expect("temp dir");
        Vault::with_file_backend(dir.path())
            .set("together", "sk-persisted")
            .expect("set");

        let reopened = Vault::with_file_backend(dir.path());
        assert_eq!(reopened.get("together").expect("get"), "sk-persisted");
    }

    #[cfg(unix)]
    #[test]
    fn the_vault_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, vault) = vault();
        vault.set("openai", "sk-value").expect("set");

        for name in ["vault.age", "vault-identity.txt"] {
            let mode = fs::metadata(dir.path().join(name))
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "{} is readable by others", name);
        }
    }
}
