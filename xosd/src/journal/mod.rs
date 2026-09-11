//! The action journal.
//!
//! The policy engine prompts before irreversible actions. Reversible ones still
//! go wrong — XOS reorganises a folder and makes it worse, or edits a file
//! badly — and prompting for those would be unbearable. So they run, and they
//! are recorded with enough state to put them back. That is what makes it
//! reasonable to give XOS a filesystem.
//!
//! # Snapshot before, not after
//!
//! [`Journal::snapshot`] must be called *before* the action happens, because it
//! captures the state the action is about to destroy. An executor that records
//! afterwards has already lost what it needed.
//!
//! # How the snapshot is taken
//!
//! Small files are copied inline into the database, which is simple and always
//! correct. Larger ones are hardlinked into the journal store, which costs no
//! extra disk because the link shares the original's data.
//!
//! That sharing has a sharp edge worth naming. A hardlink preserves content when
//! the original is unlinked or renamed, which covers deletes and moves. It does
//! *not* when the original is overwritten in place, because both names point at
//! the same inode and the bytes change underneath. So the size and modification
//! time are recorded alongside the link, and undo refuses a snapshot whose stats
//! have moved rather than restoring the wrong bytes quietly.
//!
//! # Journalling never causes a failure
//!
//! If a snapshot cannot be taken, the entry is tagged unreversible, the reason
//! is recorded, and the action proceeds. Losing the ability to undo something is
//! bad; refusing to do the work because the undo could not be prepared is worse.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tracing::warn;

/// Files at or below this are copied into the database. Above it, hardlinked.
const INLINE_LIMIT: u64 = 256 * 1024;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS actions (
    id            INTEGER PRIMARY KEY,
    at            INTEGER NOT NULL,
    tool          TEXT    NOT NULL,
    kind          TEXT    NOT NULL,
    path          TEXT    NOT NULL,
    destination   TEXT,
    goal          TEXT,
    arguments     TEXT    NOT NULL DEFAULT '',
    -- How the prior state was kept: none, absent, inline, link.
    snapshot_kind TEXT    NOT NULL,
    content       BLOB,
    link_path     TEXT,
    link_size     INTEGER,
    link_mtime    INTEGER,
    mode          INTEGER,
    mtime         INTEGER,
    reversible    INTEGER NOT NULL,
    reason        TEXT    NOT NULL DEFAULT '',
    undone_at     INTEGER
);
CREATE INDEX IF NOT EXISTS actions_by_time ON actions (at DESC);
CREATE INDEX IF NOT EXISTS actions_by_goal ON actions (goal);
";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActionKind {
    /// Creating or overwriting a file.
    Write,
    Delete,
    Move,
    Chmod,
    Mkdir,
    /// Something that left the machine. Cannot be taken back, and must have
    /// passed a policy prompt first.
    Unreversible,
}

impl ActionKind {
    pub fn label(&self) -> &'static str {
        match self {
            ActionKind::Write => "write",
            ActionKind::Delete => "delete",
            ActionKind::Move => "move",
            ActionKind::Chmod => "chmod",
            ActionKind::Mkdir => "mkdir",
            ActionKind::Unreversible => "unreversible",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "write" => Some(ActionKind::Write),
            "delete" => Some(ActionKind::Delete),
            "move" => Some(ActionKind::Move),
            "chmod" => Some(ActionKind::Chmod),
            "mkdir" => Some(ActionKind::Mkdir),
            "unreversible" => Some(ActionKind::Unreversible),
            _ => None,
        }
    }
}

/// What is about to happen.
#[derive(Debug, Clone)]
pub struct Action {
    pub tool: String,
    pub kind: ActionKind,
    pub path: PathBuf,
    /// Where a move is going.
    pub destination: Option<PathBuf>,
    /// The goal or node that asked for this.
    pub goal: Option<String>,
    pub arguments: String,
}

impl Action {
    pub fn new(tool: &str, kind: ActionKind, path: impl Into<PathBuf>) -> Self {
        Self {
            tool: tool.to_string(),
            kind,
            path: path.into(),
            destination: None,
            goal: None,
            arguments: String::new(),
        }
    }

    pub fn moving(tool: &str, from: impl Into<PathBuf>, to: impl Into<PathBuf>) -> Self {
        Self {
            destination: Some(to.into()),
            ..Action::new(tool, ActionKind::Move, from)
        }
    }

    pub fn for_goal(mut self, goal: &str) -> Self {
        self.goal = Some(goal.to_string());
        self
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub id: i64,
    pub at: i64,
    pub tool: String,
    pub kind: String,
    pub path: String,
    pub destination: Option<String>,
    pub goal: Option<String>,
    pub reversible: bool,
    pub reason: String,
    pub undone: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub goal: Option<String>,
    pub tool: Option<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub limit: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct UndoReport {
    pub undone: Vec<i64>,
    pub descriptions: Vec<String>,
    /// Set when the undo was refused before anything changed.
    pub refused: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalConfig {
    #[serde(default = "default_days")]
    pub retain_days: u64,
    #[serde(default = "default_bytes")]
    pub retain_bytes: u64,
}

fn default_days() -> u64 {
    7
}

fn default_bytes() -> u64 {
    2 * 1024 * 1024 * 1024
}

impl Default for JournalConfig {
    fn default() -> Self {
        Self {
            retain_days: default_days(),
            retain_bytes: default_bytes(),
        }
    }
}

pub struct Journal {
    connection: Mutex<Connection>,
    store: PathBuf,
    config: JournalConfig,
}

impl Journal {
    pub fn open(path: &Path, store: PathBuf, config: JournalConfig) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {}", parent.display(), e))?;
        }
        fs::create_dir_all(&store)
            .map_err(|e| format!("cannot create {}: {}", store.display(), e))?;
        let connection = Connection::open(path)
            .map_err(|e| format!("cannot open {}: {}", path.display(), e))?;
        connection
            .execute_batch(SCHEMA)
            .map_err(|e| format!("cannot prepare the journal: {}", e))?;
        Ok(Self {
            connection: Mutex::new(connection),
            store,
            config,
        })
    }

    #[cfg(test)]
    pub fn in_memory(store: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&store).map_err(|e| e.to_string())?;
        let connection = Connection::open_in_memory().map_err(|e| e.to_string())?;
        connection.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        Ok(Self {
            connection: Mutex::new(connection),
            store,
            config: JournalConfig::default(),
        })
    }

    /// Record what an action is about to destroy. Call this *before* acting.
    ///
    /// Never returns an error that should stop the action: a snapshot that
    /// cannot be taken produces an entry tagged unreversible, and the caller
    /// carries on.
    pub fn snapshot(&self, action: &Action) -> i64 {
        let now = unix_now();
        let mut snapshot_kind = "none".to_string();
        let mut content: Option<Vec<u8>> = None;
        let mut link_path: Option<String> = None;
        let mut link_size: Option<i64> = None;
        let mut link_mtime: Option<i64> = None;
        let mut mode: Option<i64> = None;
        let mut mtime: Option<i64> = None;
        let mut reversible = true;
        let mut reason = String::new();

        match action.kind {
            ActionKind::Unreversible => {
                snapshot_kind = "none".to_string();
                reversible = false;
                reason = "this action leaves the machine and cannot be taken back".to_string();
            }
            ActionKind::Mkdir => {
                // Reversing a mkdir is removing it, which needs no snapshot.
                snapshot_kind = "absent".to_string();
            }
            ActionKind::Chmod => match file_mode(&action.path) {
                Some(existing) => {
                    snapshot_kind = "absent".to_string();
                    mode = Some(existing as i64);
                }
                None => {
                    reversible = false;
                    reason = "the file's mode could not be read".to_string();
                }
            },
            ActionKind::Move => {
                // The old name is the whole snapshot.
                snapshot_kind = "absent".to_string();
            }
            ActionKind::Write | ActionKind::Delete => {
                match fs::metadata(&action.path) {
                    Err(_) => {
                        // Nothing there yet: reversing means removing it again.
                        snapshot_kind = "absent".to_string();
                    }
                    Ok(metadata) if metadata.is_dir() => {
                        reversible = false;
                        reason = "a directory cannot be snapshotted whole".to_string();
                    }
                    Ok(metadata) => {
                        mtime = modified_nanos(&metadata);
                        mode = file_mode(&action.path).map(|m| m as i64);

                        if metadata.len() <= INLINE_LIMIT {
                            match fs::read(&action.path) {
                                Ok(bytes) => {
                                    snapshot_kind = "inline".to_string();
                                    content = Some(bytes);
                                }
                                Err(error) => {
                                    reversible = false;
                                    reason = format!("the file could not be read: {}", error);
                                }
                            }
                        } else {
                            match self.link_snapshot(&action.path, now) {
                                Ok(link) => {
                                    snapshot_kind = "link".to_string();
                                    link_size = Some(metadata.len() as i64);
                                    link_mtime = mtime;
                                    link_path = Some(link);
                                }
                                Err(error) => {
                                    reversible = false;
                                    reason = format!("a snapshot could not be taken: {}", error);
                                }
                            }
                        }
                    }
                }
            }
        }

        if !reversible && !reason.is_empty() {
            // Say so, but never stop the action over it.
            warn!(
                tool = %action.tool,
                path = %action.path.display(),
                %reason,
                "recorded without a way to undo it"
            );
        }

        let outcome = (|| -> Result<i64, String> {
            let connection = self
                .connection
                .lock()
                .map_err(|_| "journal lock".to_string())?;
            connection
                .execute(
                    "INSERT INTO actions
                     (at, tool, kind, path, destination, goal, arguments, snapshot_kind,
                      content, link_path, link_size, link_mtime, mode, mtime, reversible, reason)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
                    params![
                        now,
                        action.tool,
                        action.kind.label(),
                        action.path.display().to_string(),
                        action.destination.as_ref().map(|p| p.display().to_string()),
                        action.goal,
                        action.arguments,
                        snapshot_kind,
                        content,
                        link_path,
                        link_size,
                        link_mtime,
                        mode,
                        mtime,
                        reversible as i64,
                        reason,
                    ],
                )
                .map_err(|e| e.to_string())?;
            Ok(connection.last_insert_rowid())
        })();

        match outcome {
            Ok(id) => id,
            Err(error) => {
                warn!(%error, "the action was not journalled, and proceeds anyway");
                -1
            }
        }
    }

    /// Hardlink a file into the store so its data is not duplicated.
    fn link_snapshot(&self, path: &Path, now: i64) -> Result<String, String> {
        let name = format!(
            "{}-{}",
            now,
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "file".to_string())
        );
        let target = self.store.join(name);
        fs::hard_link(path, &target).map_err(|e| e.to_string())?;
        Ok(target.display().to_string())
    }

    pub fn list(&self, filter: &Filter) -> Result<Vec<Entry>, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "journal lock".to_string())?;
        let mut sql = String::from(
            "SELECT id, at, tool, kind, path, destination, goal, reversible, reason, undone_at
             FROM actions WHERE 1=1",
        );
        if filter.goal.is_some() {
            sql.push_str(" AND goal = :goal");
        }
        if filter.tool.is_some() {
            sql.push_str(" AND tool = :tool");
        }
        if filter.since.is_some() {
            sql.push_str(" AND at >= :since");
        }
        if filter.until.is_some() {
            sql.push_str(" AND at <= :until");
        }
        sql.push_str(" ORDER BY id DESC LIMIT :limit");

        let mut statement = connection
            .prepare(&sql)
            .map_err(|e| format!("cannot read the journal: {}", e))?;

        let limit = filter.limit.max(1) as i64;
        let mut bindings: Vec<(&str, &dyn rusqlite::ToSql)> = vec![(":limit", &limit)];
        if let Some(goal) = &filter.goal {
            bindings.push((":goal", goal));
        }
        if let Some(tool) = &filter.tool {
            bindings.push((":tool", tool));
        }
        if let Some(since) = &filter.since {
            bindings.push((":since", since));
        }
        if let Some(until) = &filter.until {
            bindings.push((":until", until));
        }

        let rows = statement
            .query_map(bindings.as_slice(), |row| {
                let undone: Option<i64> = row.get(9)?;
                Ok(Entry {
                    id: row.get(0)?,
                    at: row.get(1)?,
                    tool: row.get(2)?,
                    kind: row.get(3)?,
                    path: row.get(4)?,
                    destination: row.get(5)?,
                    goal: row.get(6)?,
                    reversible: row.get::<_, i64>(7)? != 0,
                    reason: row.get(8)?,
                    undone: undone.is_some(),
                })
            })
            .map_err(|e| format!("cannot read the journal: {}", e))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("cannot read the journal: {}", e))?);
        }
        Ok(out)
    }

    /// Reverse the last `count` actions, newest first.
    ///
    /// Every step is checked before any is applied, so an undo that cannot
    /// complete does not start. A half-reversed directory is worse than one
    /// that was left alone with an explanation.
    pub fn undo(&self, count: usize) -> Result<UndoReport, String> {
        let planned = self.pending(count)?;
        if planned.is_empty() {
            return Ok(UndoReport {
                undone: Vec::new(),
                descriptions: Vec::new(),
                refused: Some("there is nothing left to undo".to_string()),
            });
        }

        // Check everything first.
        for record in &planned {
            if let Err(reason) = record.check() {
                return Ok(UndoReport {
                    undone: Vec::new(),
                    descriptions: Vec::new(),
                    refused: Some(format!(
                        "nothing was undone: {} ({}) cannot be reversed, because {}",
                        record.tool, record.path, reason
                    )),
                });
            }
        }

        let mut undone = Vec::new();
        let mut descriptions = Vec::new();
        for record in &planned {
            match record.apply() {
                Ok(description) => {
                    self.mark_undone(record.id)?;
                    undone.push(record.id);
                    descriptions.push(description);
                }
                Err(reason) => {
                    // Checked and still failed: stop here and say exactly where.
                    return Ok(UndoReport {
                        undone,
                        descriptions,
                        refused: Some(format!(
                            "stopped at {}: {}. Earlier steps were reversed.",
                            record.path, reason
                        )),
                    });
                }
            }
        }

        Ok(UndoReport {
            undone,
            descriptions,
            refused: None,
        })
    }

    /// The most recent reversible actions that have not been undone.
    fn pending(&self, count: usize) -> Result<Vec<Record>, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "journal lock".to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT id, tool, kind, path, destination, snapshot_kind, content,
                        link_path, link_size, link_mtime, mode, mtime
                 FROM actions
                 WHERE undone_at IS NULL AND reversible = 1
                 ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|e| format!("cannot read the journal: {}", e))?;

        let rows = statement
            .query_map(params![count.max(1) as i64], |row| {
                Ok(Record {
                    id: row.get(0)?,
                    tool: row.get(1)?,
                    kind: ActionKind::parse(&row.get::<_, String>(2)?)
                        .unwrap_or(ActionKind::Unreversible),
                    path: row.get(3)?,
                    destination: row.get(4)?,
                    snapshot_kind: row.get(5)?,
                    content: row.get(6)?,
                    link_path: row.get(7)?,
                    link_size: row.get(8)?,
                    link_mtime: row.get(9)?,
                    mode: row.get(10)?,
                    mtime: row.get(11)?,
                })
            })
            .map_err(|e| format!("cannot read the journal: {}", e))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("cannot read the journal: {}", e))?);
        }
        Ok(out)
    }

    fn mark_undone(&self, id: i64) -> Result<(), String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "journal lock".to_string())?;
        connection
            .execute(
                "UPDATE actions SET undone_at = ?1 WHERE id = ?2",
                params![unix_now(), id],
            )
            .map_err(|e| format!("cannot mark the action undone: {}", e))?;
        Ok(())
    }

    /// Drop entries past the retention window, oldest first.
    pub fn prune(&self) -> Result<usize, String> {
        let cutoff = unix_now() - (self.config.retain_days as i64 * 86_400);
        let links: Vec<String> = {
            let connection = self
                .connection
                .lock()
                .map_err(|_| "journal lock".to_string())?;
            let mut statement = connection
                .prepare("SELECT link_path FROM actions WHERE at < ?1 AND link_path IS NOT NULL")
                .map_err(|e| e.to_string())?;
            let rows = statement
                .query_map(params![cutoff], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            rows.filter_map(Result::ok).collect()
        };
        for link in links {
            let _ = fs::remove_file(link);
        }

        let connection = self
            .connection
            .lock()
            .map_err(|_| "journal lock".to_string())?;
        let removed = connection
            .execute("DELETE FROM actions WHERE at < ?1", params![cutoff])
            .map_err(|e| format!("cannot prune the journal: {}", e))?;
        Ok(removed)
    }

    pub fn config(&self) -> &JournalConfig {
        &self.config
    }
}

/// One journalled action, with everything needed to reverse it.
struct Record {
    id: i64,
    tool: String,
    kind: ActionKind,
    path: String,
    destination: Option<String>,
    snapshot_kind: String,
    content: Option<Vec<u8>>,
    link_path: Option<String>,
    link_size: Option<i64>,
    link_mtime: Option<i64>,
    mode: Option<i64>,
    mtime: Option<i64>,
}

impl Record {
    /// Can this be reversed right now? Checked before anything is applied.
    fn check(&self) -> Result<(), String> {
        match self.kind {
            ActionKind::Unreversible => Err("it left the machine".to_string()),
            ActionKind::Move => {
                let destination = self
                    .destination
                    .as_ref()
                    .ok_or_else(|| "the move recorded no destination".to_string())?;
                if !Path::new(destination).exists() {
                    return Err(format!("{} is no longer there", destination));
                }
                Ok(())
            }
            ActionKind::Mkdir => Ok(()),
            ActionKind::Chmod => {
                if self.mode.is_none() {
                    return Err("the previous mode was not recorded".to_string());
                }
                Ok(())
            }
            ActionKind::Write | ActionKind::Delete => match self.snapshot_kind.as_str() {
                "absent" | "inline" => Ok(()),
                "link" => {
                    let link = self
                        .link_path
                        .as_ref()
                        .ok_or_else(|| "the snapshot link is missing".to_string())?;
                    let metadata = fs::metadata(link)
                        .map_err(|_| format!("the snapshot at {} is gone", link))?;
                    // A hardlink shares its inode, so an in-place overwrite of
                    // the original changes the snapshot too. If the stats have
                    // moved, the bytes are not the ones that were recorded.
                    if Some(metadata.len() as i64) != self.link_size
                        || modified_nanos(&metadata) != self.link_mtime
                    {
                        return Err(
                            "the snapshot was changed in place, so it no longer holds the original"
                                .to_string(),
                        );
                    }
                    Ok(())
                }
                other => Err(format!("unknown snapshot kind `{}`", other)),
            },
        }
    }

    /// Put it back.
    fn apply(&self) -> Result<String, String> {
        let path = PathBuf::from(&self.path);
        match self.kind {
            ActionKind::Unreversible => Err("it left the machine".to_string()),

            ActionKind::Mkdir => match fs::remove_dir(&path) {
                Ok(()) => Ok(format!("removed the directory {}", self.path)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Ok(format!("{} was already gone", self.path))
                }
                // Something else put files here, so removing it would lose
                // them. Leaving it is the safe answer, and saying so is better
                // than a silent survivor.
                Err(error) => Ok(format!(
                    "left {} in place: {}",
                    self.path, error
                )),
            },

            ActionKind::Chmod => {
                let mode = self.mode.ok_or_else(|| "no mode recorded".to_string())?;
                set_mode(&path, mode as u32)?;
                Ok(format!("restored the mode of {}", self.path))
            }

            ActionKind::Move => {
                let destination = self
                    .destination
                    .as_ref()
                    .ok_or_else(|| "no destination recorded".to_string())?;
                if let Some(parent) = path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                fs::rename(destination, &path).map_err(|e| e.to_string())?;
                Ok(format!("moved {} back to {}", destination, self.path))
            }

            ActionKind::Write | ActionKind::Delete => match self.snapshot_kind.as_str() {
                "absent" => {
                    // There was nothing here before, so there should be nothing now.
                    match fs::remove_file(&path) {
                        Ok(()) => Ok(format!("removed {}, which did not exist before", self.path)),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            Ok(format!("{} was already gone", self.path))
                        }
                        Err(error) => Err(error.to_string()),
                    }
                }
                "inline" => {
                    let bytes = self
                        .content
                        .as_ref()
                        .ok_or_else(|| "the snapshot is empty".to_string())?;
                    if let Some(parent) = path.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    fs::write(&path, bytes).map_err(|e| e.to_string())?;
                    self.restore_stats(&path);
                    Ok(format!("restored {}", self.path))
                }
                "link" => {
                    let link = self
                        .link_path
                        .as_ref()
                        .ok_or_else(|| "no snapshot link".to_string())?;
                    if let Some(parent) = path.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let _ = fs::remove_file(&path);
                    fs::hard_link(link, &path)
                        .or_else(|_| fs::copy(link, &path).map(|_| ()))
                        .map_err(|e| e.to_string())?;
                    self.restore_stats(&path);
                    Ok(format!("restored {} from its snapshot", self.path))
                }
                other => Err(format!("unknown snapshot kind `{}`", other)),
            },
        }
    }

    /// Put the mode and modification time back, so an undone directory looks
    /// untouched rather than merely holding the right bytes.
    fn restore_stats(&self, path: &Path) {
        if let Some(mode) = self.mode {
            let _ = set_mode(path, mode as u32);
        }
        if let Some(nanos) = self.mtime {
            if let Ok(file) = fs::OpenOptions::new().write(true).open(path) {
                let when = UNIX_EPOCH + std::time::Duration::from_nanos(nanos as u64);
                let times = fs::FileTimes::new().set_modified(when);
                let _ = file.set_times(times);
            }
        }
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64
}

fn modified_nanos(metadata: &fs::Metadata) -> Option<i64> {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
}

#[cfg(unix)]
fn file_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).ok().map(|m| m.permissions().mode())
}

#[cfg(not(unix))]
fn file_mode(_path: &Path) -> Option<u32> {
    None
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        journal: Journal,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("work");
        fs::create_dir_all(&root).expect("work dir");
        let journal =
            Journal::in_memory(dir.path().join("store")).expect("journal");
        Fixture {
            _dir: dir,
            root,
            journal,
        }
    }

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent");
        }
        fs::write(path, text).expect("write");
    }

    #[test]
    fn an_overwrite_is_put_back() {
        let f = fixture();
        let file = f.root.join("notes.md");
        write(&file, "the original");

        f.journal
            .snapshot(&Action::new("write_file", ActionKind::Write, &file));
        write(&file, "clobbered");

        let report = f.journal.undo(1).expect("undo");
        assert!(report.refused.is_none(), "{:?}", report.refused);
        assert_eq!(fs::read_to_string(&file).expect("read"), "the original");
    }

    #[test]
    fn a_file_created_where_none_existed_is_removed_again() {
        let f = fixture();
        let file = f.root.join("new.txt");

        f.journal
            .snapshot(&Action::new("write_file", ActionKind::Write, &file));
        write(&file, "brand new");
        assert!(file.exists());

        f.journal.undo(1).expect("undo");
        assert!(!file.exists(), "a file that did not exist must not survive undo");
    }

    #[test]
    fn a_delete_is_restored() {
        let f = fixture();
        let file = f.root.join("gone.txt");
        write(&file, "important");

        f.journal
            .snapshot(&Action::new("delete_file", ActionKind::Delete, &file));
        fs::remove_file(&file).expect("remove");

        f.journal.undo(1).expect("undo");
        assert_eq!(fs::read_to_string(&file).expect("read"), "important");
    }

    #[test]
    fn a_move_goes_back() {
        let f = fixture();
        let from = f.root.join("a.txt");
        let to = f.root.join("sub").join("b.txt");
        write(&from, "moved me");
        fs::create_dir_all(to.parent().unwrap()).expect("dir");

        f.journal
            .snapshot(&Action::moving("move_file", &from, &to));
        fs::rename(&from, &to).expect("rename");

        f.journal.undo(1).expect("undo");
        assert!(from.exists(), "the original name must come back");
        assert!(!to.exists());
    }

    #[test]
    fn a_reorganisation_is_reversed_in_order() {
        // The check's scenario: XOS tidies a directory and makes it worse.
        let f = fixture();
        let one = f.root.join("one.txt");
        let two = f.root.join("two.txt");
        write(&one, "first");
        write(&two, "second");
        let sorted = f.root.join("sorted");
        fs::create_dir_all(&sorted).expect("dir");

        for (from, name) in [(&one, "one.txt"), (&two, "two.txt")] {
            let to = sorted.join(name);
            f.journal.snapshot(&Action::moving("move_file", from, &to));
            fs::rename(from, &to).expect("rename");
        }
        assert!(!one.exists() && !two.exists());

        let report = f.journal.undo(2).expect("undo");
        assert!(report.refused.is_none(), "{:?}", report.refused);
        assert_eq!(report.undone.len(), 2);
        assert!(one.exists() && two.exists(), "both files must come home");
        assert_eq!(fs::read_to_string(&one).expect("read"), "first");
    }

    #[test]
    fn the_modification_time_comes_back_too() {
        let f = fixture();
        let file = f.root.join("stamped.txt");
        write(&file, "original");
        let before = fs::metadata(&file).expect("metadata").modified().expect("mtime");

        f.journal
            .snapshot(&Action::new("write_file", ActionKind::Write, &file));
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&file, "changed");

        f.journal.undo(1).expect("undo");
        let after = fs::metadata(&file).expect("metadata").modified().expect("mtime");
        let drift = after
            .duration_since(before)
            .unwrap_or_else(|e| e.duration())
            .as_millis();
        assert!(drift < 5, "the timestamp moved by {}ms", drift);
    }

    #[test]
    fn a_large_file_is_hardlinked_rather_than_copied() {
        let f = fixture();
        let file = f.root.join("big.bin");
        write(&file, &"x".repeat((INLINE_LIMIT + 1024) as usize));

        f.journal
            .snapshot(&Action::new("delete_file", ActionKind::Delete, &file));
        let links = fs::read_dir(&f.journal.store)
            .expect("store")
            .filter_map(Result::ok)
            .count();
        assert_eq!(links, 1, "a large file should be linked into the store");

        fs::remove_file(&file).expect("remove");
        f.journal.undo(1).expect("undo");
        assert!(file.exists());
        assert_eq!(
            fs::metadata(&file).expect("metadata").len(),
            INLINE_LIMIT + 1024
        );
    }

    #[test]
    fn an_undo_that_cannot_finish_does_not_start() {
        let f = fixture();
        let one = f.root.join("safe.txt");
        write(&one, "safe");
        f.journal
            .snapshot(&Action::new("write_file", ActionKind::Write, &one));
        write(&one, "changed");

        // A move whose destination someone else has since removed.
        let from = f.root.join("from.txt");
        let to = f.root.join("to.txt");
        write(&from, "x");
        f.journal.snapshot(&Action::moving("move_file", &from, &to));
        fs::rename(&from, &to).expect("rename");
        fs::remove_file(&to).expect("someone removed it");

        let report = f.journal.undo(2).expect("undo");
        assert!(report.refused.is_some(), "the undo should refuse");
        assert!(report.undone.is_empty(), "nothing may be applied");
        assert_eq!(
            fs::read_to_string(&one).expect("read"),
            "changed",
            "an undo that cannot finish must leave everything alone"
        );
    }

    #[test]
    fn journalling_never_stops_an_action() {
        let f = fixture();
        // A path that cannot be read: the snapshot fails, the entry records why.
        let missing = f.root.join("nope").join("deep").join("file.txt");
        let id = f
            .journal
            .snapshot(&Action::new("write_file", ActionKind::Write, &missing));
        assert!(id > 0, "the entry must still be written");

        let entries = f
            .journal
            .list(&Filter {
                limit: 10,
                ..Default::default()
            })
            .expect("list");
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn an_unreversible_action_is_tagged_and_never_undone() {
        let f = fixture();
        f.journal.snapshot(&Action::new(
            "send_email",
            ActionKind::Unreversible,
            f.root.join("outbox"),
        ));

        let entries = f
            .journal
            .list(&Filter {
                limit: 10,
                ..Default::default()
            })
            .expect("list");
        assert!(!entries[0].reversible);
        assert!(entries[0].reason.contains("cannot be taken back"));

        let report = f.journal.undo(1).expect("undo");
        assert!(
            report.refused.is_some(),
            "an unreversible action is not a candidate for undo"
        );
    }

    #[test]
    fn actions_inside_one_second_still_undo_in_reverse_order() {
        // Timestamps have one-second resolution, so ordering by time alone
        // would undo a directory before the files moved into it.
        let f = fixture();
        let made = f.root.join("sorted");
        f.journal
            .snapshot(&Action::new("make_directory", ActionKind::Mkdir, &made));
        fs::create_dir_all(&made).expect("dir");

        let from = f.root.join("a.txt");
        let to = made.join("a.txt");
        write(&from, "contents");
        f.journal.snapshot(&Action::moving("move_file", &from, &to));
        fs::rename(&from, &to).expect("rename");

        let report = f.journal.undo(2).expect("undo");
        assert!(report.refused.is_none(), "{:?}", report.refused);
        assert!(from.exists(), "the file must come back");
        assert!(
            !made.exists(),
            "the directory must be removed after it is emptied, not before"
        );
    }

    #[test]
    fn undoing_twice_does_not_repeat_itself() {
        let f = fixture();
        let file = f.root.join("once.txt");
        write(&file, "original");
        f.journal
            .snapshot(&Action::new("write_file", ActionKind::Write, &file));
        write(&file, "changed");

        f.journal.undo(1).expect("first undo");
        let second = f.journal.undo(1).expect("second undo");
        assert!(second.refused.is_some(), "nothing should be left to undo");
    }

    #[test]
    fn the_journal_filters_by_goal_and_tool() {
        let f = fixture();
        let file = f.root.join("a.txt");
        write(&file, "x");
        f.journal.snapshot(
            &Action::new("write_file", ActionKind::Write, &file).for_goal("tidy-downloads"),
        );
        f.journal
            .snapshot(&Action::new("chmod_file", ActionKind::Chmod, &file).for_goal("other"));

        let tidy = f
            .journal
            .list(&Filter {
                goal: Some("tidy-downloads".to_string()),
                limit: 10,
                ..Default::default()
            })
            .expect("list");
        assert_eq!(tidy.len(), 1);
        assert_eq!(tidy[0].tool, "write_file");

        let by_tool = f
            .journal
            .list(&Filter {
                tool: Some("chmod_file".to_string()),
                limit: 10,
                ..Default::default()
            })
            .expect("list");
        assert_eq!(by_tool.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_permission_change_is_put_back() {
        use std::os::unix::fs::PermissionsExt;
        let f = fixture();
        let file = f.root.join("script.sh");
        write(&file, "#!/bin/sh\n");
        set_mode(&file, 0o644).expect("mode");

        f.journal
            .snapshot(&Action::new("chmod_file", ActionKind::Chmod, &file));
        set_mode(&file, 0o777).expect("mode");

        f.journal.undo(1).expect("undo");
        let mode = fs::metadata(&file).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o644);
    }
}
