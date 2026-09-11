//! XOS Memory — three-tier persistent memory.
//!
//! Holds conversation, working and long-term context in SQLite, and answers
//! retrieval queries that give the model continuity across sessions.
//!
//! # The three tiers
//!
//! *Working* is the current task: minutes to hours, active context, tool results
//! and scratch. *Session* is the current day: the conversation, files touched,
//! decisions made. *Long-term* persists: preferences, projects, contacts,
//! workflows, compiled prompts.
//!
//! Promotion is explicit, never automatic drift. Closing a task summarises
//! working into session; closing a day distils session into long-term. The
//! summary is produced by the local model, so nothing leaves the machine to make
//! it.
//!
//! Memory is local-only and is never transmitted wholesale. Anything bound for a
//! cloud API goes through the digest builder, not raw recall.

pub mod bundle;
pub mod embed;

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use embed::EmbedderConfig;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS memory (
    id        INTEGER PRIMARY KEY,
    at        INTEGER NOT NULL,
    tier      TEXT    NOT NULL,
    content   TEXT    NOT NULL,
    tags      TEXT    NOT NULL DEFAULT '',
    source    TEXT    NOT NULL DEFAULT '',
    embedding BLOB,
    -- Identifies the same fact written twice, so an import can merge.
    digest    TEXT    NOT NULL UNIQUE
);
CREATE INDEX IF NOT EXISTS memory_by_tier ON memory (tier, at DESC);
";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    Working,
    Session,
    LongTerm,
}

impl Tier {
    pub fn label(&self) -> &'static str {
        match self {
            Tier::Working => "working",
            Tier::Session => "session",
            Tier::LongTerm => "long-term",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_lowercase().replace('_', "-").as_str() {
            "working" => Some(Tier::Working),
            "session" => Some(Tier::Session),
            "long-term" | "longterm" | "long" => Some(Tier::LongTerm),
            _ => None,
        }
    }

    pub fn all() -> [Tier; 3] {
        [Tier::Working, Tier::Session, Tier::LongTerm]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: i64,
    pub at: i64,
    pub tier: String,
    pub content: String,
    pub tags: String,
    pub source: String,
    pub digest: String,
}

/// An entry with its recall score.
#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    #[serde(flatten)]
    pub entry: Entry,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub tier: String,
    pub entries: i64,
    pub oldest: Option<i64>,
    pub newest: Option<i64>,
}

pub struct Memory {
    connection: Mutex<Connection>,
    embedder: EmbedderConfig,
}

impl Memory {
    pub fn open(path: &Path, embedder: EmbedderConfig) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {}", parent.display(), e))?;
        }
        let connection = Connection::open(path)
            .map_err(|e| format!("cannot open {}: {}", path.display(), e))?;
        connection
            .execute_batch(SCHEMA)
            .map_err(|e| format!("cannot prepare memory: {}", e))?;
        Ok(Self {
            connection: Mutex::new(connection),
            embedder,
        })
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self, String> {
        let connection = Connection::open_in_memory().map_err(|e| e.to_string())?;
        connection.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        Ok(Self {
            connection: Mutex::new(connection),
            embedder: EmbedderConfig::Lexical,
        })
    }

    pub fn embedder(&self) -> &EmbedderConfig {
        &self.embedder
    }

    /// Write something worth remembering. Returns the row id, or the existing
    /// one when the same content is already held in that tier.
    pub fn write(
        &self,
        tier: Tier,
        content: &str,
        tags: &str,
        source: &str,
    ) -> Result<i64, String> {
        let content = content.trim();
        if content.is_empty() {
            return Err("nothing to remember".to_string());
        }
        let digest = digest_of(tier, content);
        let vector = embed::to_bytes(&embed::lexical(&format!("{} {}", content, tags)));
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0) as i64;

        let connection = self.connection.lock().map_err(|_| "memory lock".to_string())?;
        connection
            .execute(
                "INSERT INTO memory (at, tier, content, tags, source, embedding, digest)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(digest) DO NOTHING",
                params![now, tier.label(), content, tags, source, vector, digest],
            )
            .map_err(|e| format!("cannot write memory: {}", e))?;

        connection
            .query_row(
                "SELECT id FROM memory WHERE digest = ?1",
                params![digest],
                |row| row.get(0),
            )
            .map_err(|e| format!("cannot write memory: {}", e))
    }

    /// Rank entries against a query.
    ///
    /// Vector similarity does the ranking; a literal substring match adds a
    /// bonus, because when someone types an exact path or name they mean it.
    pub fn recall(&self, query: &str, tier: Option<Tier>, limit: usize) -> Result<Vec<Hit>, String> {
        let wanted = embed::lexical(query);
        let needle = query.trim().to_lowercase();

        let connection = self.connection.lock().map_err(|_| "memory lock".to_string())?;
        let sql = match tier {
            Some(_) => {
                "SELECT id, at, tier, content, tags, source, digest, embedding
                 FROM memory WHERE tier = ?1"
            }
            None => {
                "SELECT id, at, tier, content, tags, source, digest, embedding
                 FROM memory WHERE ?1 IS NULL OR 1=1"
            }
        };
        let mut statement = connection
            .prepare(sql)
            .map_err(|e| format!("cannot search memory: {}", e))?;

        let bind = tier.map(|t| t.label().to_string());
        let rows = statement
            .query_map(params![bind], |row| {
                let embedding: Option<Vec<u8>> = row.get(7)?;
                Ok((
                    Entry {
                        id: row.get(0)?,
                        at: row.get(1)?,
                        tier: row.get(2)?,
                        content: row.get(3)?,
                        tags: row.get(4)?,
                        source: row.get(5)?,
                        digest: row.get(6)?,
                    },
                    embedding,
                ))
            })
            .map_err(|e| format!("cannot search memory: {}", e))?;

        let mut hits = Vec::new();
        for row in rows {
            let (entry, embedding) = row.map_err(|e| format!("cannot search memory: {}", e))?;
            let stored = embedding.map(|bytes| embed::from_bytes(&bytes)).unwrap_or_default();
            let mut score = embed::similarity(&wanted, &stored);
            if !needle.is_empty() && entry.content.to_lowercase().contains(&needle) {
                score += 0.5;
            }
            if score > 0.0 {
                hits.push(Hit { entry, score });
            }
        }

        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(limit.max(1));
        Ok(hits)
    }

    /// Everything currently held in a tier, oldest first. Used when building a
    /// summary for promotion.
    pub fn entries(&self, tier: Tier) -> Result<Vec<Entry>, String> {
        let connection = self.connection.lock().map_err(|_| "memory lock".to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT id, at, tier, content, tags, source, digest
                 FROM memory WHERE tier = ?1 ORDER BY at ASC, id ASC",
            )
            .map_err(|e| format!("cannot read memory: {}", e))?;
        let rows = statement
            .query_map(params![tier.label()], |row| {
                Ok(Entry {
                    id: row.get(0)?,
                    at: row.get(1)?,
                    tier: row.get(2)?,
                    content: row.get(3)?,
                    tags: row.get(4)?,
                    source: row.get(5)?,
                    digest: row.get(6)?,
                })
            })
            .map_err(|e| format!("cannot read memory: {}", e))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("cannot read memory: {}", e))?);
        }
        Ok(out)
    }

    /// Clear a tier, having promoted what matters out of it.
    pub fn clear(&self, tier: Tier) -> Result<usize, String> {
        let connection = self.connection.lock().map_err(|_| "memory lock".to_string())?;
        connection
            .execute("DELETE FROM memory WHERE tier = ?1", params![tier.label()])
            .map_err(|e| format!("cannot clear memory: {}", e))
    }

    pub fn stats(&self) -> Result<Vec<Stats>, String> {
        let connection = self.connection.lock().map_err(|_| "memory lock".to_string())?;
        let mut out = Vec::new();
        for tier in Tier::all() {
            let row = connection
                .query_row(
                    "SELECT COUNT(*), MIN(at), MAX(at) FROM memory WHERE tier = ?1",
                    params![tier.label()],
                    |row| {
                        Ok(Stats {
                            tier: tier.label().to_string(),
                            entries: row.get(0)?,
                            oldest: row.get(1)?,
                            newest: row.get(2)?,
                        })
                    },
                )
                .map_err(|e| format!("cannot read memory stats: {}", e))?;
            out.push(row);
        }
        Ok(out)
    }

    /// Write a promoted summary into the next tier up and clear the one below.
    ///
    /// The summary text is produced by the caller, which is the only part that
    /// needs a model. Memory itself never talks to a provider.
    pub fn promote(&self, from: Tier, to: Tier, summary: &str) -> Result<i64, String> {
        let id = self.write(
            to,
            summary,
            &format!("promoted,{}", from.label()),
            "promotion",
        )?;
        self.clear(from)?;
        Ok(id)
    }

    /// Every entry, for an export bundle.
    pub fn all(&self) -> Result<Vec<Entry>, String> {
        let mut out = Vec::new();
        for tier in Tier::all() {
            out.extend(self.entries(tier)?);
        }
        Ok(out)
    }

    /// Merge entries from a bundle. Existing rows are kept; only entries this
    /// machine has never seen are added, so importing twice changes nothing.
    pub fn merge(&self, entries: &[Entry]) -> Result<usize, String> {
        let known: HashSet<String> = self
            .all()?
            .into_iter()
            .map(|entry| entry.digest)
            .collect();

        let mut added = 0;
        for entry in entries {
            if known.contains(&entry.digest) {
                continue;
            }
            let tier = Tier::parse(&entry.tier).unwrap_or(Tier::LongTerm);
            let vector = embed::to_bytes(&embed::lexical(&format!(
                "{} {}",
                entry.content, entry.tags
            )));
            let connection = self.connection.lock().map_err(|_| "memory lock".to_string())?;
            let changed = connection
                .execute(
                    "INSERT INTO memory (at, tier, content, tags, source, embedding, digest)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(digest) DO NOTHING",
                    params![
                        entry.at,
                        tier.label(),
                        entry.content,
                        entry.tags,
                        entry.source,
                        vector,
                        entry.digest
                    ],
                )
                .map_err(|e| format!("cannot merge memory: {}", e))?;
            added += changed;
        }
        Ok(added)
    }
}

/// Identifies the same content in the same tier, so a re-import is a no-op.
fn digest_of(tier: Tier, content: &str) -> String {
    let mut value: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in tier.label().as_bytes().iter().chain(content.as_bytes()) {
        value ^= *byte as u64;
        value = value.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:016x}", value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> Memory {
        Memory::in_memory().expect("memory")
    }

    #[test]
    fn something_written_can_be_recalled() {
        let memory = memory();
        memory
            .write(Tier::Working, "the backup job runs at 02:00", "backup", "test")
            .expect("write");

        let hits = memory.recall("backup job", None, 5).expect("recall");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].entry.content.contains("02:00"));
        assert!(hits[0].score > 0.0);
    }

    #[test]
    fn recall_ranks_the_better_match_first() {
        let memory = memory();
        memory
            .write(Tier::LongTerm, "I prefer dark themes everywhere", "ui", "test")
            .expect("write");
        memory
            .write(Tier::LongTerm, "the nightly backup job runs at 02:00", "backup", "test")
            .expect("write");

        let hits = memory.recall("nightly backup", None, 5).expect("recall");
        assert!(
            hits[0].entry.content.contains("backup"),
            "got {:?}",
            hits[0].entry.content
        );
    }

    #[test]
    fn recall_can_be_held_to_one_tier() {
        let memory = memory();
        memory.write(Tier::Working, "scratch note about ssh", "", "test").expect("write");
        memory.write(Tier::LongTerm, "ssh key lives in .ssh", "", "test").expect("write");

        let hits = memory.recall("ssh", Some(Tier::LongTerm), 5).expect("recall");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.tier, "long-term");
    }

    #[test]
    fn writing_the_same_thing_twice_stores_it_once() {
        let memory = memory();
        let first = memory.write(Tier::Session, "same fact", "", "test").expect("write");
        let again = memory.write(Tier::Session, "same fact", "", "test").expect("write");
        assert_eq!(first, again);
        assert_eq!(memory.entries(Tier::Session).expect("entries").len(), 1);
    }

    #[test]
    fn the_same_text_in_two_tiers_is_two_entries() {
        let memory = memory();
        memory.write(Tier::Working, "a fact", "", "test").expect("write");
        memory.write(Tier::Session, "a fact", "", "test").expect("write");
        assert_eq!(memory.entries(Tier::Working).expect("entries").len(), 1);
        assert_eq!(memory.entries(Tier::Session).expect("entries").len(), 1);
    }

    #[test]
    fn empty_content_is_refused() {
        assert!(memory().write(Tier::Working, "   ", "", "test").is_err());
    }

    #[test]
    fn promotion_moves_a_summary_up_and_clears_below() {
        let memory = memory();
        memory.write(Tier::Working, "opened the config", "", "test").expect("write");
        memory.write(Tier::Working, "changed the socket path", "", "test").expect("write");

        memory
            .promote(Tier::Working, Tier::Session, "Adjusted the daemon socket path.")
            .expect("promote");

        assert!(
            memory.entries(Tier::Working).expect("entries").is_empty(),
            "working must be cleared once promoted"
        );
        let session = memory.entries(Tier::Session).expect("entries");
        assert_eq!(session.len(), 1);
        assert!(session[0].content.contains("socket path"));
        assert!(session[0].tags.contains("promoted"));
    }

    #[test]
    fn a_promoted_summary_is_recallable() {
        let memory = memory();
        memory.write(Tier::Working, "scratch", "", "test").expect("write");
        memory
            .promote(Tier::Working, Tier::Session, "Fixed the nightly backup job.")
            .expect("promote");

        let hits = memory.recall("backup job", Some(Tier::Session), 5).expect("recall");
        assert_eq!(hits.len(), 1, "the summary must be findable after promotion");
    }

    #[test]
    fn stats_cover_every_tier() {
        let memory = memory();
        memory.write(Tier::Working, "one", "", "test").expect("write");
        let stats = memory.stats().expect("stats");
        assert_eq!(stats.len(), 3);
        let working = stats.iter().find(|s| s.tier == "working").expect("working");
        assert_eq!(working.entries, 1);
    }

    #[test]
    fn merging_adds_only_what_is_new() {
        let memory = memory();
        memory.write(Tier::LongTerm, "already known", "", "test").expect("write");
        let existing = memory.all().expect("all");

        let mut incoming = existing.clone();
        incoming.push(Entry {
            id: 0,
            at: 1,
            tier: "long-term".to_string(),
            content: "brought in from a bundle".to_string(),
            tags: String::new(),
            source: "import".to_string(),
            digest: digest_of(Tier::LongTerm, "brought in from a bundle"),
        });

        let added = memory.merge(&incoming).expect("merge");
        assert_eq!(added, 1, "only the unseen entry should be added");
        assert_eq!(memory.all().expect("all").len(), 2);
    }

    #[test]
    fn importing_the_same_bundle_twice_changes_nothing() {
        let memory = memory();
        memory.write(Tier::LongTerm, "a fact", "", "test").expect("write");
        let bundle = memory.all().expect("all");

        assert_eq!(memory.merge(&bundle).expect("merge"), 0);
        assert_eq!(memory.merge(&bundle).expect("merge"), 0);
        assert_eq!(memory.all().expect("all").len(), 1);
    }

    #[test]
    fn merging_never_overwrites_what_is_already_here() {
        let memory = memory();
        memory.write(Tier::LongTerm, "mine", "local", "test").expect("write");

        let incoming = vec![Entry {
            id: 99,
            at: 1,
            tier: "long-term".to_string(),
            content: "mine".to_string(),
            tags: "theirs".to_string(),
            source: "import".to_string(),
            digest: digest_of(Tier::LongTerm, "mine"),
        }];
        memory.merge(&incoming).expect("merge");

        let entries = memory.entries(Tier::LongTerm).expect("entries");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].tags, "local", "the local copy must win");
    }
}
