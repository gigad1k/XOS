//! Per-provider spend, persisted to SQLite and queryable by day.
//!
//! Caps are checked before a request starts, never during one. A provider that
//! has spent its budget becomes unavailable, which the caller learns when it
//! asks what is available; it does not discover the cap halfway through a reply.
//! Days are UTC and computed by SQLite, so the daemon carries no calendar logic.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS spend (
    id         INTEGER PRIMARY KEY,
    at         INTEGER NOT NULL,
    provider   TEXT    NOT NULL,
    tokens_in  INTEGER NOT NULL,
    tokens_out INTEGER NOT NULL,
    cost       REAL    NOT NULL
);
CREATE INDEX IF NOT EXISTS spend_by_day ON spend (at, provider);
";

/// Daily ceilings, in whatever currency the provider costs are quoted in.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Caps {
    /// Across every provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_total: Option<f64>,
    /// Per provider name.
    #[serde(default)]
    pub daily_per_provider: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Availability {
    Available,
    /// The provider has spent its budget for today.
    OverCap {
        scope: String,
        spent: f64,
        cap: f64,
    },
}

impl Availability {
    pub fn is_available(&self) -> bool {
        matches!(self, Availability::Available)
    }

    /// A sentence that says what to do, with no apology.
    pub fn reason(&self) -> Option<String> {
        match self {
            Availability::Available => None,
            Availability::OverCap { scope, spent, cap } => Some(format!(
                "the {} daily cap is spent: {:.4} of {:.4}. Raise it in the config or wait for tomorrow.",
                scope, spent, cap
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DayRow {
    pub day: String,
    pub provider: String,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cost: f64,
}

pub struct SpendBook {
    connection: Mutex<Connection>,
    caps: Caps,
}

impl SpendBook {
    pub fn open(path: &Path, caps: Caps) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {}", parent.display(), e))?;
        }
        let connection = Connection::open(path)
            .map_err(|e| format!("cannot open {}: {}", path.display(), e))?;
        connection
            .execute_batch(SCHEMA)
            .map_err(|e| format!("cannot prepare the spend database: {}", e))?;
        Ok(Self {
            connection: Mutex::new(connection),
            caps,
        })
    }

    #[cfg(test)]
    pub fn in_memory(caps: Caps) -> Result<Self, String> {
        let connection = Connection::open_in_memory().map_err(|e| e.to_string())?;
        connection.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        Ok(Self {
            connection: Mutex::new(connection),
            caps,
        })
    }

    pub fn caps(&self) -> &Caps {
        &self.caps
    }

    /// Record what one completion cost.
    pub fn record(
        &self,
        provider: &str,
        tokens_in: u32,
        tokens_out: u32,
        cost: f64,
    ) -> Result<(), String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0) as i64;
        let connection = self.connection.lock().map_err(|_| "spend lock".to_string())?;
        connection
            .execute(
                "INSERT INTO spend (at, provider, tokens_in, tokens_out, cost)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![now, provider, tokens_in as i64, tokens_out as i64, cost],
            )
            .map_err(|e| format!("cannot record spend: {}", e))?;
        Ok(())
    }

    /// What one provider has spent today, UTC.
    pub fn spent_today(&self, provider: &str) -> Result<f64, String> {
        let connection = self.connection.lock().map_err(|_| "spend lock".to_string())?;
        connection
            .query_row(
                "SELECT COALESCE(SUM(cost), 0.0) FROM spend
                 WHERE provider = ?1 AND date(at, 'unixepoch') = date('now')",
                params![provider],
                |row| row.get(0),
            )
            .map_err(|e| format!("cannot read spend: {}", e))
    }

    /// What every provider has spent today, UTC.
    pub fn spent_today_total(&self) -> Result<f64, String> {
        let connection = self.connection.lock().map_err(|_| "spend lock".to_string())?;
        connection
            .query_row(
                "SELECT COALESCE(SUM(cost), 0.0) FROM spend
                 WHERE date(at, 'unixepoch') = date('now')",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("cannot read spend: {}", e))
    }

    /// Spend grouped by day and provider, newest first.
    pub fn by_day(&self, days: u32) -> Result<Vec<DayRow>, String> {
        let connection = self.connection.lock().map_err(|_| "spend lock".to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT date(at, 'unixepoch') AS day, provider,
                        SUM(tokens_in), SUM(tokens_out), SUM(cost)
                 FROM spend
                 WHERE at >= strftime('%s', 'now', ?1)
                 GROUP BY day, provider
                 ORDER BY day DESC, provider ASC",
            )
            .map_err(|e| format!("cannot read spend: {}", e))?;

        let window = format!("-{} days", days.max(1));
        let rows = statement
            .query_map(params![window], |row| {
                Ok(DayRow {
                    day: row.get(0)?,
                    provider: row.get(1)?,
                    tokens_in: row.get(2)?,
                    tokens_out: row.get(3)?,
                    cost: row.get(4)?,
                })
            })
            .map_err(|e| format!("cannot read spend: {}", e))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("cannot read spend: {}", e))?);
        }
        Ok(out)
    }

    /// Whether a provider may be used right now. Checked before a request, so a
    /// cap never interrupts a reply in progress.
    pub fn availability(&self, provider: &str) -> Availability {
        if let Some(cap) = self.caps.daily_total {
            let spent = self.spent_today_total().unwrap_or(0.0);
            if spent >= cap {
                return Availability::OverCap {
                    scope: "global".to_string(),
                    spent,
                    cap,
                };
            }
        }
        if let Some(cap) = self.caps.daily_per_provider.get(provider).copied() {
            let spent = self.spent_today(provider).unwrap_or(0.0);
            if spent >= cap {
                return Availability::OverCap {
                    scope: format!("`{}`", provider),
                    spent,
                    cap,
                };
            }
        }
        Availability::Available
    }
}

/// What a completion cost, from the provider's quoted rates.
pub fn cost_of(tokens_in: u32, tokens_out: u32, per_1k_in: f64, per_1k_out: f64) -> f64 {
    (tokens_in as f64 / 1000.0) * per_1k_in + (tokens_out as f64 / 1000.0) * per_1k_out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(caps: Caps) -> SpendBook {
        SpendBook::in_memory(caps).expect("spend book")
    }

    #[test]
    fn cost_follows_the_quoted_rates() {
        // 1000 in at 0.003, 500 out at 0.015
        let cost = cost_of(1000, 500, 0.003, 0.015);
        assert!((cost - (0.003 + 0.0075)).abs() < 1e-9, "got {}", cost);
    }

    #[test]
    fn a_local_provider_costs_nothing() {
        assert_eq!(cost_of(9999, 9999, 0.0, 0.0), 0.0);
    }

    #[test]
    fn spend_accumulates_for_today() {
        let book = book(Caps::default());
        book.record("openrouter", 1000, 500, 0.25).expect("record");
        book.record("openrouter", 1000, 500, 0.25).expect("record");
        let spent = book.spent_today("openrouter").expect("spent");
        assert!((spent - 0.5).abs() < 1e-9, "got {}", spent);
    }

    #[test]
    fn providers_are_counted_apart_but_also_together() {
        let book = book(Caps::default());
        book.record("openrouter", 10, 10, 0.10).expect("record");
        book.record("anthropic", 10, 10, 0.40).expect("record");
        assert!((book.spent_today("openrouter").unwrap() - 0.10).abs() < 1e-9);
        assert!((book.spent_today_total().unwrap() - 0.50).abs() < 1e-9);
    }

    #[test]
    fn everything_is_available_without_caps() {
        let book = book(Caps::default());
        book.record("openrouter", 10, 10, 1000.0).expect("record");
        assert!(book.availability("openrouter").is_available());
    }

    #[test]
    fn a_provider_cap_makes_that_provider_unavailable() {
        let mut caps = Caps::default();
        caps.daily_per_provider
            .insert("openrouter".to_string(), 1.0);
        let book = book(caps);

        book.record("openrouter", 10, 10, 0.5).expect("record");
        assert!(book.availability("openrouter").is_available());

        book.record("openrouter", 10, 10, 0.6).expect("record");
        let availability = book.availability("openrouter");
        assert!(!availability.is_available());
        assert!(availability.reason().expect("reason").contains("daily cap"));
        // Another provider is untouched by one provider's cap.
        assert!(book.availability("anthropic").is_available());
    }

    #[test]
    fn the_global_cap_stops_every_provider() {
        let caps = Caps {
            daily_total: Some(1.0),
            ..Default::default()
        };
        let book = book(caps);
        book.record("anthropic", 10, 10, 1.5).expect("record");
        assert!(!book.availability("openrouter").is_available());
        assert!(!book.availability("anthropic").is_available());
    }

    #[test]
    fn the_reason_names_the_scope_and_the_numbers() {
        let caps = Caps {
            daily_total: Some(2.0),
            ..Default::default()
        };
        let book = book(caps);
        book.record("groq", 1, 1, 3.0).expect("record");
        let reason = book.availability("groq").reason().expect("reason");
        assert!(reason.contains("global"), "{}", reason);
        assert!(reason.contains("3.0000"), "{}", reason);
    }

    #[test]
    fn by_day_groups_and_reports_tokens() {
        let book = book(Caps::default());
        book.record("openrouter", 100, 50, 0.01).expect("record");
        book.record("openrouter", 200, 80, 0.02).expect("record");
        let rows = book.by_day(7).expect("by day");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].provider, "openrouter");
        assert_eq!(rows[0].tokens_in, 300);
        assert_eq!(rows[0].tokens_out, 130);
        assert!((rows[0].cost - 0.03).abs() < 1e-9);
    }
}
