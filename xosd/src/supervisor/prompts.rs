//! Compiled prompts, and the guard that stops them drifting.
//!
//! Prompt compilation is the supervisor's highest-leverage job: it writes tight,
//! task-specific instructions for the local model once, and the local model
//! executes them hundreds of times. Each compiled prompt carries a stable
//! `cache_key`, so llama.cpp pins its prefix to a slot and stops reprocessing
//! it. Better instructions and no cost for sending them compound.
//!
//! # Why a recompile is not adopted on faith
//!
//! Recompiling changes system behaviour silently. A prompt that reads better can
//! score worse, and nobody would notice until autonomy had quietly degraded and
//! the reason was months back. So a replacement is scored against the incumbent
//! on real cases before it is adopted, both scores are logged, and a candidate
//! that does not beat the prompt it replaces is discarded.
//!
//! The first compilation for a task type has nothing to regress against, so it
//! is adopted and marked unguarded. A *replacement* with no cases to score on is
//! refused, because that is exactly the drift the guard exists to prevent.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde::Serialize;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS prompts (
    task_type   TEXT PRIMARY KEY,
    body        TEXT    NOT NULL,
    cache_key   TEXT    NOT NULL,
    compiled_at INTEGER NOT NULL,
    version     INTEGER NOT NULL DEFAULT 1,
    hits        INTEGER NOT NULL DEFAULT 0,
    failures    INTEGER NOT NULL DEFAULT 0,
    score       REAL,
    guarded     INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS prompt_history (
    id           INTEGER PRIMARY KEY,
    at           INTEGER NOT NULL,
    task_type    TEXT    NOT NULL,
    outcome      TEXT    NOT NULL,
    old_score    REAL,
    new_score    REAL,
    note         TEXT    NOT NULL DEFAULT ''
);
";

#[derive(Debug, Clone, Serialize)]
pub struct CompiledPrompt {
    pub task_type: String,
    pub body: String,
    /// Passed to the local provider so its KV cache stays warm. Changes on every
    /// recompile, which is what evicts the old prefix.
    pub cache_key: String,
    pub compiled_at: i64,
    pub version: i64,
    pub hits: i64,
    pub failures: i64,
    pub score: Option<f64>,
    /// False when the prompt was adopted without being scored.
    pub guarded: bool,
}

impl CompiledPrompt {
    /// Share of executions that went wrong. Drives recompilation.
    pub fn failure_rate(&self) -> f64 {
        let total = self.hits + self.failures;
        if total == 0 {
            0.0
        } else {
            self.failures as f64 / total as f64
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryEntry {
    pub at: i64,
    pub task_type: String,
    pub outcome: String,
    pub old_score: Option<f64>,
    pub new_score: Option<f64>,
    pub note: String,
}

/// What the guard decided about a candidate prompt.
#[derive(Debug, Clone, PartialEq)]
pub enum Adoption {
    /// Nothing existed before, so there was nothing to regress from.
    FirstCompile,
    /// It scored better than the prompt it replaces.
    Improved { old: f64, new: f64 },
    /// It did not, so the incumbent stays.
    Rejected { old: f64, new: f64 },
    /// A replacement with no way to score it. Refused rather than guessed.
    Unverifiable { reason: String },
}

impl Adoption {
    pub fn adopts(&self) -> bool {
        matches!(self, Adoption::FirstCompile | Adoption::Improved { .. })
    }

    pub fn label(&self) -> &'static str {
        match self {
            Adoption::FirstCompile => "first-compile",
            Adoption::Improved { .. } => "improved",
            Adoption::Rejected { .. } => "rejected",
            Adoption::Unverifiable { .. } => "unverifiable",
        }
    }
}

pub struct PromptCache {
    connection: Mutex<Connection>,
}

impl PromptCache {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {}", parent.display(), e))?;
        }
        let connection = Connection::open(path)
            .map_err(|e| format!("cannot open {}: {}", path.display(), e))?;
        connection
            .execute_batch(SCHEMA)
            .map_err(|e| format!("cannot prepare the prompt cache: {}", e))?;
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

    pub fn get(&self, task_type: &str) -> Option<CompiledPrompt> {
        let connection = self.connection.lock().ok()?;
        connection
            .query_row(
                "SELECT task_type, body, cache_key, compiled_at, version, hits, failures, score, guarded
                 FROM prompts WHERE task_type = ?1",
                params![task_type],
                |row| {
                    Ok(CompiledPrompt {
                        task_type: row.get(0)?,
                        body: row.get(1)?,
                        cache_key: row.get(2)?,
                        compiled_at: row.get(3)?,
                        version: row.get(4)?,
                        hits: row.get(5)?,
                        failures: row.get(6)?,
                        score: row.get(7)?,
                        guarded: row.get::<_, i64>(8)? != 0,
                    })
                },
            )
            .ok()
    }

    pub fn list(&self) -> Result<Vec<CompiledPrompt>, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "prompt cache lock".to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT task_type, body, cache_key, compiled_at, version, hits, failures, score, guarded
                 FROM prompts ORDER BY task_type",
            )
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok(CompiledPrompt {
                    task_type: row.get(0)?,
                    body: row.get(1)?,
                    cache_key: row.get(2)?,
                    compiled_at: row.get(3)?,
                    version: row.get(4)?,
                    hits: row.get(5)?,
                    failures: row.get(6)?,
                    score: row.get(7)?,
                    guarded: row.get::<_, i64>(8)? != 0,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    /// Adopt a candidate, if the guard allows it.
    ///
    /// `candidate_score` and the incumbent's score decide. Both are written to
    /// the history either way, so a rejection is as visible as an adoption.
    pub fn adopt(
        &self,
        task_type: &str,
        body: &str,
        candidate_score: Option<f64>,
    ) -> Result<Adoption, String> {
        let existing = self.get(task_type);

        let decision = match (&existing, candidate_score) {
            (None, _) => Adoption::FirstCompile,
            (Some(current), Some(new)) => {
                let old = current.score.unwrap_or(0.0);
                if new > old {
                    Adoption::Improved { old, new }
                } else {
                    Adoption::Rejected { old, new }
                }
            }
            (Some(_), None) => Adoption::Unverifiable {
                reason: "a replacement cannot be scored, so the prompt in use stays".to_string(),
            },
        };

        let now = unix_now();
        if decision.adopts() {
            let version = existing.as_ref().map(|p| p.version + 1).unwrap_or(1);
            // A new key evicts the old prefix from the provider's slot.
            let cache_key = format!("{}-v{}", task_type, version);
            let connection = self
                .connection
                .lock()
                .map_err(|_| "prompt cache lock".to_string())?;
            connection
                .execute(
                    "INSERT INTO prompts
                     (task_type, body, cache_key, compiled_at, version, hits, failures, score, guarded)
                     VALUES (?1, ?2, ?3, ?4, ?5, 0, 0, ?6, ?7)
                     ON CONFLICT(task_type) DO UPDATE SET
                       body = excluded.body,
                       cache_key = excluded.cache_key,
                       compiled_at = excluded.compiled_at,
                       version = excluded.version,
                       hits = 0,
                       failures = 0,
                       score = excluded.score,
                       guarded = excluded.guarded",
                    params![
                        task_type,
                        body,
                        cache_key,
                        now,
                        version,
                        candidate_score,
                        candidate_score.is_some() as i64
                    ],
                )
                .map_err(|e| format!("cannot store the prompt: {}", e))?;
        }

        let (old_score, new_score, note) = match &decision {
            Adoption::FirstCompile => (
                None,
                candidate_score,
                "first prompt for this task type".to_string(),
            ),
            Adoption::Improved { old, new } => {
                (Some(*old), Some(*new), "adopted".to_string())
            }
            Adoption::Rejected { old, new } => (
                Some(*old),
                Some(*new),
                "the candidate scored no better, so the prompt in use stays".to_string(),
            ),
            Adoption::Unverifiable { reason } => (
                existing.as_ref().and_then(|p| p.score),
                None,
                reason.clone(),
            ),
        };

        let connection = self
            .connection
            .lock()
            .map_err(|_| "prompt cache lock".to_string())?;
        connection
            .execute(
                "INSERT INTO prompt_history (at, task_type, outcome, old_score, new_score, note)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![now, task_type, decision.label(), old_score, new_score, note],
            )
            .map_err(|e| format!("cannot record the prompt history: {}", e))?;

        Ok(decision)
    }

    /// Record a freshly measured score for the prompt in use.
    ///
    /// Bookkeeping, not a decision: re-measuring the incumbent so a comparison
    /// is like for like must not look like an adoption in the history.
    pub fn set_score(&self, task_type: &str, score: f64) -> Result<(), String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "prompt cache lock".to_string())?;
        connection
            .execute(
                "UPDATE prompts SET score = ?1 WHERE task_type = ?2",
                params![score, task_type],
            )
            .map_err(|e| format!("cannot update the score: {}", e))?;
        Ok(())
    }

    /// Count one execution of a cached prompt.
    pub fn record_use(&self, task_type: &str, succeeded: bool) -> Result<(), String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "prompt cache lock".to_string())?;
        let column = if succeeded { "hits" } else { "failures" };
        connection
            .execute(
                &format!(
                    "UPDATE prompts SET {} = {} + 1 WHERE task_type = ?1",
                    column, column
                ),
                params![task_type],
            )
            .map_err(|e| format!("cannot record prompt use: {}", e))?;
        Ok(())
    }

    pub fn history(&self, limit: u32) -> Result<Vec<HistoryEntry>, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "prompt cache lock".to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT at, task_type, outcome, old_score, new_score, note
                 FROM prompt_history ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map(params![limit.max(1)], |row| {
                Ok(HistoryEntry {
                    at: row.get(0)?,
                    task_type: row.get(1)?,
                    outcome: row.get(2)?,
                    old_score: row.get(3)?,
                    new_score: row.get(4)?,
                    note: row.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> PromptCache {
        PromptCache::in_memory().expect("cache")
    }

    #[test]
    fn a_first_prompt_is_adopted() {
        let cache = cache();
        let decision = cache.adopt("tidy-files", "instructions", None).expect("adopt");
        assert_eq!(decision, Adoption::FirstCompile);

        let stored = cache.get("tidy-files").expect("stored");
        assert_eq!(stored.body, "instructions");
        assert_eq!(stored.version, 1);
        assert!(!stored.guarded, "an unscored first compile is not guarded");
    }

    #[test]
    fn a_better_candidate_replaces_the_incumbent() {
        let cache = cache();
        cache.adopt("tidy-files", "v1", Some(0.6)).expect("adopt");
        let decision = cache.adopt("tidy-files", "v2", Some(0.8)).expect("adopt");

        assert_eq!(decision, Adoption::Improved { old: 0.6, new: 0.8 });
        let stored = cache.get("tidy-files").expect("stored");
        assert_eq!(stored.body, "v2");
        assert_eq!(stored.version, 2);
    }

    #[test]
    fn a_worse_candidate_is_discarded() {
        let cache = cache();
        cache.adopt("tidy-files", "good", Some(0.9)).expect("adopt");
        let decision = cache.adopt("tidy-files", "worse", Some(0.5)).expect("adopt");

        assert_eq!(decision, Adoption::Rejected { old: 0.9, new: 0.5 });
        assert_eq!(
            cache.get("tidy-files").expect("stored").body,
            "good",
            "the prompt in use must survive a worse candidate"
        );
    }

    #[test]
    fn an_equal_candidate_is_not_adopted() {
        // Churn without improvement is still drift.
        let cache = cache();
        cache.adopt("tidy-files", "v1", Some(0.7)).expect("adopt");
        let decision = cache.adopt("tidy-files", "v2", Some(0.7)).expect("adopt");
        assert!(!decision.adopts());
    }

    #[test]
    fn an_unscorable_replacement_is_refused() {
        let cache = cache();
        cache.adopt("tidy-files", "v1", Some(0.7)).expect("adopt");
        let decision = cache.adopt("tidy-files", "v2", None).expect("adopt");

        assert!(matches!(decision, Adoption::Unverifiable { .. }));
        assert_eq!(cache.get("tidy-files").expect("stored").body, "v1");
    }

    #[test]
    fn both_scores_are_logged_either_way() {
        let cache = cache();
        cache.adopt("tidy-files", "v1", Some(0.6)).expect("adopt");
        cache.adopt("tidy-files", "v2", Some(0.4)).expect("adopt");

        let history = cache.history(10).expect("history");
        assert_eq!(history.len(), 2);
        let rejection = &history[0];
        assert_eq!(rejection.outcome, "rejected");
        assert_eq!(rejection.old_score, Some(0.6));
        assert_eq!(rejection.new_score, Some(0.4));
    }

    #[test]
    fn a_recompile_changes_the_cache_key_so_the_old_prefix_is_evicted() {
        let cache = cache();
        cache.adopt("tidy-files", "v1", Some(0.5)).expect("adopt");
        let first = cache.get("tidy-files").expect("stored").cache_key;

        cache.adopt("tidy-files", "v2", Some(0.9)).expect("adopt");
        let second = cache.get("tidy-files").expect("stored").cache_key;

        assert_ne!(first, second, "a stale prefix must not be reused");
    }

    #[test]
    fn use_counts_drive_the_failure_rate() {
        let cache = cache();
        cache.adopt("tidy-files", "v1", None).expect("adopt");
        for _ in 0..8 {
            cache.record_use("tidy-files", true).expect("record");
        }
        for _ in 0..2 {
            cache.record_use("tidy-files", false).expect("record");
        }

        let stored = cache.get("tidy-files").expect("stored");
        assert_eq!(stored.hits, 8);
        assert_eq!(stored.failures, 2);
        assert!((stored.failure_rate() - 0.2).abs() < 1e-9);
    }

    #[test]
    fn stats_reset_when_a_prompt_is_replaced() {
        let cache = cache();
        cache.adopt("tidy-files", "v1", Some(0.5)).expect("adopt");
        cache.record_use("tidy-files", false).expect("record");
        cache.adopt("tidy-files", "v2", Some(0.9)).expect("adopt");

        let stored = cache.get("tidy-files").expect("stored");
        assert_eq!(stored.failures, 0, "old failures belong to the old prompt");
    }

    #[test]
    fn rescoring_the_incumbent_writes_no_history() {
        let cache = cache();
        cache.adopt("tidy", "v1", Some(0.5)).expect("adopt");
        cache.set_score("tidy", 0.7).expect("set score");

        assert_eq!(cache.get("tidy").expect("stored").score, Some(0.7));
        assert_eq!(
            cache.history(10).expect("history").len(),
            1,
            "re-measuring is not a decision and must not read like one"
        );
    }

    #[test]
    fn an_unknown_task_type_has_no_prompt() {
        assert!(cache().get("never-seen").is_none());
    }
}
