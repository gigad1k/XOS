//! Every policy decision, recorded.
//!
//! A capability firewall is only trustworthy if you can see what it did. This
//! is the record: what was asked, what was decided, and why.
//!
//! Arguments are stored redacted. A log of blocked secret reads would otherwise
//! become the thing worth stealing.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde::Serialize;

use super::{redact, PolicyDecision};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS decisions (
    id        INTEGER PRIMARY KEY,
    at        INTEGER NOT NULL,
    tool      TEXT    NOT NULL,
    arguments TEXT    NOT NULL,
    decision  TEXT    NOT NULL,
    reason    TEXT    NOT NULL,
    source    TEXT    NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS decisions_by_time ON decisions (at DESC);
";

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub id: i64,
    pub at: i64,
    pub tool: String,
    pub arguments: String,
    pub decision: String,
    pub reason: String,
    pub source: String,
}

pub struct PolicyLog {
    connection: Mutex<Connection>,
}

impl PolicyLog {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {}", parent.display(), e))?;
        }
        let connection = Connection::open(path)
            .map_err(|e| format!("cannot open {}: {}", path.display(), e))?;
        connection
            .execute_batch(SCHEMA)
            .map_err(|e| format!("cannot prepare the policy log: {}", e))?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self, String> {
        let connection = Connection::open_in_memory().map_err(|e| e.to_string())?;
        connection.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn record(
        &self,
        tool: &str,
        arguments: &str,
        decision: &PolicyDecision,
        source: &str,
    ) -> Result<(), String> {
        // The log is not a place to keep secrets safe from the rules above it.
        let (safe_arguments, _) = redact::redact(arguments);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0) as i64;

        let connection = self
            .connection
            .lock()
            .map_err(|_| "policy log lock".to_string())?;
        connection
            .execute(
                "INSERT INTO decisions (at, tool, arguments, decision, reason, source)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    now,
                    tool,
                    safe_arguments,
                    decision.label(),
                    decision.reason(),
                    source
                ],
            )
            .map_err(|e| format!("cannot record the decision: {}", e))?;
        Ok(())
    }

    pub fn recent(&self, limit: u32) -> Result<Vec<Entry>, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "policy log lock".to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT id, at, tool, arguments, decision, reason, source
                 FROM decisions ORDER BY at DESC, id DESC LIMIT ?1",
            )
            .map_err(|e| format!("cannot read the policy log: {}", e))?;
        let rows = statement
            .query_map(params![limit.max(1)], |row| {
                Ok(Entry {
                    id: row.get(0)?,
                    at: row.get(1)?,
                    tool: row.get(2)?,
                    arguments: row.get(3)?,
                    decision: row.get(4)?,
                    reason: row.get(5)?,
                    source: row.get(6)?,
                })
            })
            .map_err(|e| format!("cannot read the policy log: {}", e))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("cannot read the policy log: {}", e))?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decision_is_recorded_with_its_reason() {
        let log = PolicyLog::in_memory().expect("log");
        log.record(
            "read_file",
            r#"{"path":"/home/me/.ssh/id_rsa"}"#,
            &PolicyDecision::Block {
                reason: ".ssh holds credentials".to_string(),
            },
            "chat",
        )
        .expect("record");

        let entries = log.recent(10).expect("recent");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].decision, "block");
        assert!(entries[0].reason.contains(".ssh"));
        assert_eq!(entries[0].source, "chat");
    }

    #[test]
    fn arguments_are_redacted_before_they_are_logged() {
        let log = PolicyLog::in_memory().expect("log");
        log.record(
            "post_message",
            r#"{"body":"the key is sk-abcdefghijklmnop1234567890"}"#,
            &PolicyDecision::Allow,
            "chat",
        )
        .expect("record");

        let entries = log.recent(10).expect("recent");
        assert!(
            !entries[0].arguments.contains("sk-abcdef"),
            "the log must not become the thing worth stealing: {}",
            entries[0].arguments
        );
        assert!(entries[0].arguments.contains("[redacted:"));
    }

    #[test]
    fn newest_comes_first() {
        let log = PolicyLog::in_memory().expect("log");
        log.record("first", "{}", &PolicyDecision::Allow, "chat").expect("record");
        log.record("second", "{}", &PolicyDecision::Allow, "chat").expect("record");
        let entries = log.recent(10).expect("recent");
        assert_eq!(entries[0].tool, "second");
    }

    #[test]
    fn every_decision_kind_can_be_logged() {
        let log = PolicyLog::in_memory().expect("log");
        let decisions = [
            PolicyDecision::Allow,
            PolicyDecision::Prompt {
                reason: "cannot be undone".to_string(),
            },
            PolicyDecision::Block {
                reason: "secret path".to_string(),
            },
            PolicyDecision::Redact { spans: Vec::new() },
        ];
        for decision in &decisions {
            log.record("tool", "{}", decision, "test").expect("record");
        }
        assert_eq!(log.recent(10).expect("recent").len(), 4);
    }
}
