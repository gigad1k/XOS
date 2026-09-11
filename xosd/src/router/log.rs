//! The escalation log.
//!
//! Every time a request leaves the machine, a row lands here saying why. This
//! is the record you read when the bill is larger than expected, or when you
//! want to know whether the local model is actually carrying its share.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde::Serialize;

use super::{TaskClass, Trigger};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS escalations (
    id           INTEGER PRIMARY KEY,
    at           INTEGER NOT NULL,
    trigger      TEXT    NOT NULL,
    task_class   TEXT    NOT NULL,
    local_model  TEXT    NOT NULL,
    target_model TEXT    NOT NULL,
    tokens_in    INTEGER NOT NULL,
    tokens_out   INTEGER NOT NULL,
    cost         REAL    NOT NULL,
    outcome      TEXT    NOT NULL,
    reason       TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS escalations_by_time ON escalations (at DESC);
";

/// One escalation, as it happened.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub id: i64,
    pub at: i64,
    pub trigger: String,
    pub task_class: String,
    pub local_model: String,
    pub target_model: String,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cost: f64,
    pub outcome: String,
    pub reason: String,
}

/// What to write when an escalation happens.
#[derive(Debug, Clone)]
pub struct Record {
    pub trigger: Trigger,
    pub task_class: TaskClass,
    pub local_model: String,
    pub target_model: String,
    pub tokens_in: u32,
    pub tokens_out: u32,
    pub cost: f64,
    pub outcome: String,
    pub reason: String,
}

pub struct EscalationLog {
    connection: Mutex<Connection>,
}

impl EscalationLog {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {}", parent.display(), e))?;
        }
        let connection = Connection::open(path)
            .map_err(|e| format!("cannot open {}: {}", path.display(), e))?;
        connection
            .execute_batch(SCHEMA)
            .map_err(|e| format!("cannot prepare the escalation log: {}", e))?;
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

    pub fn record(&self, record: &Record) -> Result<(), String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0) as i64;
        let connection = self
            .connection
            .lock()
            .map_err(|_| "escalation log lock".to_string())?;
        connection
            .execute(
                "INSERT INTO escalations
                 (at, trigger, task_class, local_model, target_model,
                  tokens_in, tokens_out, cost, outcome, reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    now,
                    record.trigger.label(),
                    record.task_class.label(),
                    record.local_model,
                    record.target_model,
                    record.tokens_in as i64,
                    record.tokens_out as i64,
                    record.cost,
                    record.outcome,
                    record.reason,
                ],
            )
            .map_err(|e| format!("cannot record the escalation: {}", e))?;
        Ok(())
    }

    /// Most recent first.
    pub fn recent(&self, limit: u32) -> Result<Vec<Entry>, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "escalation log lock".to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT id, at, trigger, task_class, local_model, target_model,
                        tokens_in, tokens_out, cost, outcome, reason
                 FROM escalations ORDER BY at DESC, id DESC LIMIT ?1",
            )
            .map_err(|e| format!("cannot read the escalation log: {}", e))?;

        let rows = statement
            .query_map(params![limit.max(1)], |row| {
                Ok(Entry {
                    id: row.get(0)?,
                    at: row.get(1)?,
                    trigger: row.get(2)?,
                    task_class: row.get(3)?,
                    local_model: row.get(4)?,
                    target_model: row.get(5)?,
                    tokens_in: row.get(6)?,
                    tokens_out: row.get(7)?,
                    cost: row.get(8)?,
                    outcome: row.get(9)?,
                    reason: row.get(10)?,
                })
            })
            .map_err(|e| format!("cannot read the escalation log: {}", e))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("cannot read the escalation log: {}", e))?);
        }
        Ok(out)
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(trigger: Trigger) -> Record {
        Record {
            trigger,
            task_class: TaskClass::Chat,
            local_model: "gemma4:e4b".to_string(),
            target_model: "openrouter".to_string(),
            tokens_in: 100,
            tokens_out: 50,
            cost: 0.01,
            outcome: "ok".to_string(),
            reason: "a reason".to_string(),
        }
    }

    #[test]
    fn an_escalation_is_recorded_with_its_trigger() {
        let log = EscalationLog::in_memory().expect("log");
        log.record(&record(Trigger::ContextOverflow)).expect("record");

        let entries = log.recent(10).expect("recent");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].trigger, "context-overflow");
        assert_eq!(entries[0].task_class, "chat");
        assert_eq!(entries[0].target_model, "openrouter");
        assert!((entries[0].cost - 0.01).abs() < 1e-9);
    }

    #[test]
    fn every_trigger_round_trips_through_its_label() {
        let log = EscalationLog::in_memory().expect("log");
        let triggers = [
            Trigger::SchemaFailure,
            Trigger::TaskClass,
            Trigger::ContextOverflow,
            Trigger::Throughput,
            Trigger::ToolComplexity,
            Trigger::LowConfidence,
            Trigger::CostMode,
            Trigger::Manual,
        ];
        for trigger in triggers {
            log.record(&record(trigger)).expect("record");
        }

        let entries = log.recent(100).expect("recent");
        assert_eq!(entries.len(), 8, "every trigger must be recordable");
        let labels: Vec<&str> = entries.iter().map(|e| e.trigger.as_str()).collect();
        for trigger in triggers {
            assert!(
                labels.contains(&trigger.label()),
                "{} is missing from the log",
                trigger.label()
            );
        }
    }

    #[test]
    fn recent_returns_newest_first_and_respects_the_limit() {
        let log = EscalationLog::in_memory().expect("log");
        log.record(&record(Trigger::Manual)).expect("record");
        log.record(&record(Trigger::CostMode)).expect("record");

        let entries = log.recent(1).expect("recent");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].trigger, "cost-mode", "newest first");
    }
}
