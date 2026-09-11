//! XOS Vault — credentials held in the system keyring.
//!
//! Brokers secrets to subsystems that need them. Credentials are never written
//! to the repository, the journal or any API-bound context.
