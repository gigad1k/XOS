//! XOS Goals — long-running goal decomposition and execution.
//!
//! Stores goals as a task graph, runs nodes on the local model, and reports
//! node-level progress so slow agent work reads as working rather than hung.
//!
//! This is what makes XOS an operator rather than an assistant: a goal is
//! declared once and worked at over days, surviving reboots because the graph
//! lives in SQLite rather than in a process.
//!
//! # This module never calls a provider
//!
//! It holds state and decides what is eligible to run. Execution is driven from
//! the daemon, which owns the router and the policy engine, so a node cannot
//! reach a model without passing both. There is no provider handle in here to
//! misuse.

pub mod conditions;

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

pub use conditions::{Condition, SystemState};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS goals (
    id          TEXT PRIMARY KEY,
    description TEXT    NOT NULL,
    state       TEXT    NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS nodes (
    id            TEXT PRIMARY KEY,
    goal_id       TEXT    NOT NULL,
    title         TEXT    NOT NULL,
    detail        TEXT    NOT NULL DEFAULT '',
    state         TEXT    NOT NULL,
    position      INTEGER NOT NULL,
    attempts      INTEGER NOT NULL DEFAULT 0,
    retry_budget  INTEGER NOT NULL DEFAULT 2,
    conditions    TEXT    NOT NULL DEFAULT '',
    result        TEXT,
    failure       TEXT,
    -- A person said yes to this node's prompt. Cleared once it has been used,
    -- so one approval covers one attempt rather than all future ones.
    confirmed     INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    FOREIGN KEY (goal_id) REFERENCES goals (id)
);
CREATE TABLE IF NOT EXISTS edges (
    goal_id  TEXT NOT NULL,
    before   TEXT NOT NULL,
    after    TEXT NOT NULL,
    PRIMARY KEY (before, after)
);
CREATE TABLE IF NOT EXISTS transitions (
    id       INTEGER PRIMARY KEY,
    at       INTEGER NOT NULL,
    node_id  TEXT NOT NULL,
    was      TEXT NOT NULL,
    now      TEXT NOT NULL,
    note     TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS nodes_by_goal ON nodes (goal_id, position);
";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeState {
    /// Ready to run once its dependencies are done.
    Pending,
    /// Waiting on another node's result.
    Blocked,
    Running,
    /// An irreversible action is waiting for a person.
    NeedsUser,
    Done,
    /// Out of retries. The supervisor re-plans from here.
    Failed,
}

impl NodeState {
    pub fn label(&self) -> &'static str {
        match self {
            NodeState::Pending => "pending",
            NodeState::Blocked => "blocked",
            NodeState::Running => "running",
            NodeState::NeedsUser => "needs-user",
            NodeState::Done => "done",
            NodeState::Failed => "failed",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "pending" => Some(NodeState::Pending),
            "blocked" => Some(NodeState::Blocked),
            "running" => Some(NodeState::Running),
            "needs-user" => Some(NodeState::NeedsUser),
            "done" => Some(NodeState::Done),
            "failed" => Some(NodeState::Failed),
            _ => None,
        }
    }

    pub fn settled(&self) -> bool {
        matches!(self, NodeState::Done | NodeState::Failed)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Goal {
    pub id: String,
    pub description: String,
    pub state: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub id: String,
    pub goal_id: String,
    pub title: String,
    pub detail: String,
    pub state: String,
    pub position: i64,
    pub attempts: i64,
    pub retry_budget: i64,
    pub conditions: Vec<Condition>,
    pub result: Option<String>,
    pub failure: Option<String>,
    /// A person has approved this node's prompt.
    pub confirmed: bool,
}

impl Node {
    pub fn node_state(&self) -> NodeState {
        NodeState::parse(&self.state).unwrap_or(NodeState::Pending)
    }

    /// Out of retries, so the supervisor is the next step rather than another go.
    pub fn exhausted(&self) -> bool {
        self.attempts > self.retry_budget
    }
}

/// A node as the supervisor described it, before it is stored.
#[derive(Debug, Clone, Deserialize)]
pub struct NodeSpec {
    pub title: String,
    #[serde(default)]
    pub detail: String,
    /// Titles this node waits on.
    #[serde(default)]
    pub after: Vec<String>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
    #[serde(default = "default_retries")]
    pub retry_budget: i64,
}

fn default_retries() -> i64 {
    2
}

pub struct Graph {
    connection: Mutex<Connection>,
}

impl Graph {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {}", parent.display(), e))?;
        }
        let connection = Connection::open(path)
            .map_err(|e| format!("cannot open {}: {}", path.display(), e))?;
        connection
            .execute_batch(SCHEMA)
            .map_err(|e| format!("cannot prepare the graph: {}", e))?;
        // A node left running by a crash is not running now.
        connection
            .execute(
                "UPDATE nodes SET state = 'pending' WHERE state = 'running'",
                [],
            )
            .map_err(|e| format!("cannot recover the graph: {}", e))?;
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

    /// Reset nodes a crash left mid-flight. Called on startup.
    pub fn recover(&self) -> Result<usize, String> {
        let connection = self.connection.lock().map_err(|_| "graph lock".to_string())?;
        connection
            .execute(
                "UPDATE nodes SET state = 'pending' WHERE state = 'running'",
                [],
            )
            .map_err(|e| format!("cannot recover the graph: {}", e))
    }

    pub fn create_goal(&self, description: &str) -> Result<String, String> {
        let id = short_id("goal", description);
        let now = unix_now();
        let connection = self.connection.lock().map_err(|_| "graph lock".to_string())?;
        connection
            .execute(
                "INSERT INTO goals (id, description, state, created_at, updated_at)
                 VALUES (?1, ?2, 'planning', ?3, ?3)",
                params![id, description, now],
            )
            .map_err(|e| format!("cannot create the goal: {}", e))?;
        Ok(id)
    }

    /// Store a decomposition. Replaces any nodes the goal already had, which is
    /// what a re-plan does.
    pub fn plan(&self, goal_id: &str, specs: &[NodeSpec]) -> Result<usize, String> {
        if specs.is_empty() {
            return Err("a plan with no steps is not a plan".to_string());
        }
        let now = unix_now();
        let mut connection = self.connection.lock().map_err(|_| "graph lock".to_string())?;
        let transaction = connection
            .transaction()
            .map_err(|e| format!("cannot plan: {}", e))?;

        // A re-plan replaces what has not finished; finished work is kept.
        transaction
            .execute(
                "DELETE FROM edges WHERE goal_id = ?1",
                params![goal_id],
            )
            .map_err(|e| e.to_string())?;
        transaction
            .execute(
                "DELETE FROM nodes WHERE goal_id = ?1 AND state != 'done'",
                params![goal_id],
            )
            .map_err(|e| e.to_string())?;

        let mut ids = Vec::new();
        for (position, spec) in specs.iter().enumerate() {
            let node_id = short_id("node", &format!("{}{}{}", goal_id, position, spec.title));
            let conditions = serde_json::to_string(&spec.conditions).unwrap_or_default();
            transaction
                .execute(
                    "INSERT INTO nodes
                     (id, goal_id, title, detail, state, position, attempts, retry_budget,
                      conditions, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, 'pending', ?5, 0, ?6, ?7, ?8, ?8)",
                    params![
                        node_id,
                        goal_id,
                        spec.title,
                        spec.detail,
                        position as i64,
                        spec.retry_budget,
                        conditions,
                        now
                    ],
                )
                .map_err(|e| format!("cannot store a node: {}", e))?;
            ids.push((spec.title.clone(), node_id));
        }

        // Dependencies are declared by title, which is what the supervisor
        // naturally produces.
        for (position, spec) in specs.iter().enumerate() {
            let after = &ids[position].1;
            for dependency in &spec.after {
                if let Some((_, before)) = ids.iter().find(|(title, _)| title == dependency) {
                    transaction
                        .execute(
                            "INSERT OR IGNORE INTO edges (goal_id, before, after)
                             VALUES (?1, ?2, ?3)",
                            params![goal_id, before, after],
                        )
                        .map_err(|e| format!("cannot store an edge: {}", e))?;
                }
            }
        }

        transaction
            .execute(
                "UPDATE goals SET state = 'running', updated_at = ?1 WHERE id = ?2",
                params![now, goal_id],
            )
            .map_err(|e| e.to_string())?;
        transaction
            .commit()
            .map_err(|e| format!("cannot plan: {}", e))?;
        // The lock is not reentrant, and refresh_blocked takes it again. Holding
        // it here would deadlock the daemon, not just this call.
        drop(connection);

        self.refresh_blocked(goal_id)?;
        Ok(specs.len())
    }

    /// Put every node into blocked or pending depending on its dependencies.
    pub fn refresh_blocked(&self, goal_id: &str) -> Result<(), String> {
        let nodes = self.nodes(goal_id)?;
        for node in &nodes {
            if node.node_state().settled() || node.node_state() == NodeState::Running {
                continue;
            }
            let waiting = self.unfinished_dependencies(&node.id)?;
            let wanted = if waiting.is_empty() {
                NodeState::Pending
            } else {
                NodeState::Blocked
            };
            if node.node_state() != wanted {
                self.set_state(&node.id, wanted, "dependencies rechecked")?;
            }
        }
        Ok(())
    }

    fn unfinished_dependencies(&self, node_id: &str) -> Result<Vec<String>, String> {
        let connection = self.connection.lock().map_err(|_| "graph lock".to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT n.id FROM edges e JOIN nodes n ON n.id = e.before
                 WHERE e.after = ?1 AND n.state != 'done'",
            )
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map(params![node_id], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    /// Nodes that could run right now, given the machine's state.
    ///
    /// Conditions are checked here rather than at execution time, so a node that
    /// wants AC power simply is not eligible on battery instead of failing.
    pub fn eligible(&self, state: &SystemState) -> Result<Vec<Node>, String> {
        let connection = self.connection.lock().map_err(|_| "graph lock".to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT id, goal_id, title, detail, state, position, attempts, retry_budget,
                        conditions, result, failure, confirmed
                 FROM nodes WHERE state = 'pending' ORDER BY goal_id, position",
            )
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], read_node)
            .map_err(|e| e.to_string())?;

        let mut out = Vec::new();
        for row in rows {
            let node: Node = row.map_err(|e| e.to_string())?;
            if node.conditions.iter().all(|c| c.met(state)) {
                out.push(node);
            }
        }
        Ok(out)
    }

    pub fn nodes(&self, goal_id: &str) -> Result<Vec<Node>, String> {
        let connection = self.connection.lock().map_err(|_| "graph lock".to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT id, goal_id, title, detail, state, position, attempts, retry_budget,
                        conditions, result, failure, confirmed
                 FROM nodes WHERE goal_id = ?1 ORDER BY position",
            )
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map(params![goal_id], read_node)
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    pub fn node(&self, node_id: &str) -> Option<Node> {
        let connection = self.connection.lock().ok()?;
        connection
            .query_row(
                "SELECT id, goal_id, title, detail, state, position, attempts, retry_budget,
                        conditions, result, failure, confirmed
                 FROM nodes WHERE id = ?1",
                params![node_id],
                read_node,
            )
            .ok()
    }

    pub fn goals(&self) -> Result<Vec<Goal>, String> {
        let connection = self.connection.lock().map_err(|_| "graph lock".to_string())?;
        let mut statement = connection
            .prepare("SELECT id, description, state, created_at, updated_at FROM goals ORDER BY created_at DESC")
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok(Goal {
                    id: row.get(0)?,
                    description: row.get(1)?,
                    state: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    pub fn goal(&self, goal_id: &str) -> Option<Goal> {
        self.goals()
            .ok()?
            .into_iter()
            .find(|goal| goal.id == goal_id)
    }

    pub fn set_state(&self, node_id: &str, state: NodeState, note: &str) -> Result<(), String> {
        let was = self
            .node(node_id)
            .map(|node| node.state)
            .unwrap_or_else(|| "unknown".to_string());
        let now = unix_now();
        let connection = self.connection.lock().map_err(|_| "graph lock".to_string())?;
        connection
            .execute(
                "UPDATE nodes SET state = ?1, updated_at = ?2 WHERE id = ?3",
                params![state.label(), now, node_id],
            )
            .map_err(|e| format!("cannot move the node: {}", e))?;
        connection
            .execute(
                "INSERT INTO transitions (at, node_id, was, now, note) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![now, node_id, was, state.label(), note],
            )
            .map_err(|e| format!("cannot record the transition: {}", e))?;
        Ok(())
    }

    /// Mark a node as started, counting the attempt.
    pub fn start(&self, node_id: &str) -> Result<(), String> {
        {
            let connection = self.connection.lock().map_err(|_| "graph lock".to_string())?;
            connection
                .execute(
                    "UPDATE nodes SET attempts = attempts + 1 WHERE id = ?1",
                    params![node_id],
                )
                .map_err(|e| e.to_string())?;
        }
        self.set_state(node_id, NodeState::Running, "started")
    }

    pub fn finish(&self, node_id: &str, result: &str) -> Result<(), String> {
        {
            let connection = self.connection.lock().map_err(|_| "graph lock".to_string())?;
            connection
                .execute(
                    "UPDATE nodes SET result = ?1 WHERE id = ?2",
                    params![result, node_id],
                )
                .map_err(|e| e.to_string())?;
        }
        self.set_state(node_id, NodeState::Done, "finished")?;
        if let Some(node) = self.node(node_id) {
            self.refresh_blocked(&node.goal_id)?;
            self.settle_goal(&node.goal_id)?;
        }
        Ok(())
    }

    /// Record a failure. A node with retries left goes back to pending; one
    /// without is failed, which is what asks the supervisor to re-plan.
    pub fn fail(&self, node_id: &str, failure: &str) -> Result<NodeState, String> {
        {
            let connection = self.connection.lock().map_err(|_| "graph lock".to_string())?;
            connection
                .execute(
                    "UPDATE nodes SET failure = ?1 WHERE id = ?2",
                    params![failure, node_id],
                )
                .map_err(|e| e.to_string())?;
        }
        let node = self
            .node(node_id)
            .ok_or_else(|| "the node is gone".to_string())?;

        if node.exhausted() {
            self.set_state(node_id, NodeState::Failed, failure)?;
            self.settle_goal(&node.goal_id)?;
            Ok(NodeState::Failed)
        } else {
            self.set_state(
                node_id,
                NodeState::Pending,
                &format!("attempt {} failed, retrying", node.attempts),
            )?;
            Ok(NodeState::Pending)
        }
    }

    pub fn needs_user(&self, node_id: &str, reason: &str) -> Result<(), String> {
        self.set_state(node_id, NodeState::NeedsUser, reason)
    }

    /// A person approved the prompt. The node becomes runnable and carries the
    /// approval, so the next attempt goes ahead instead of asking again.
    ///
    /// Without this an approval does nothing: policy would re-evaluate on the
    /// next attempt, prompt again, and the node would sit in needs-user forever.
    pub fn approve(&self, node_id: &str) -> Result<(), String> {
        {
            let connection = self.connection.lock().map_err(|_| "graph lock".to_string())?;
            connection
                .execute(
                    "UPDATE nodes SET confirmed = 1 WHERE id = ?1",
                    params![node_id],
                )
                .map_err(|e| format!("cannot approve the node: {}", e))?;
        }
        self.set_state(node_id, NodeState::Pending, "approved by a person")
    }

    /// Spend the approval. One yes covers one attempt.
    pub fn clear_confirmation(&self, node_id: &str) -> Result<(), String> {
        let connection = self.connection.lock().map_err(|_| "graph lock".to_string())?;
        connection
            .execute(
                "UPDATE nodes SET confirmed = 0 WHERE id = ?1",
                params![node_id],
            )
            .map_err(|e| format!("cannot clear the approval: {}", e))?;
        Ok(())
    }

    /// Move a goal to done or failed once its nodes have settled.
    fn settle_goal(&self, goal_id: &str) -> Result<(), String> {
        let nodes = self.nodes(goal_id)?;
        if nodes.is_empty() {
            return Ok(());
        }
        let state = if nodes.iter().all(|n| n.node_state() == NodeState::Done) {
            "done"
        } else if nodes.iter().any(|n| n.node_state() == NodeState::Failed) {
            "needs-replan"
        } else {
            "running"
        };
        let connection = self.connection.lock().map_err(|_| "graph lock".to_string())?;
        connection
            .execute(
                "UPDATE goals SET state = ?1, updated_at = ?2 WHERE id = ?3",
                params![state, unix_now(), goal_id],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Nodes that ran out of retries, which the supervisor re-plans from.
    pub fn failed_nodes(&self) -> Result<Vec<Node>, String> {
        let connection = self.connection.lock().map_err(|_| "graph lock".to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT id, goal_id, title, detail, state, position, attempts, retry_budget,
                        conditions, result, failure, confirmed
                 FROM nodes WHERE state = 'failed' ORDER BY goal_id, position",
            )
            .map_err(|e| e.to_string())?;
        let rows = statement.query_map([], read_node).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }
}

fn read_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<Node> {
    let conditions: String = row.get(8)?;
    Ok(Node {
        id: row.get(0)?,
        goal_id: row.get(1)?,
        title: row.get(2)?,
        detail: row.get(3)?,
        state: row.get(4)?,
        position: row.get(5)?,
        attempts: row.get(6)?,
        retry_budget: row.get(7)?,
        conditions: serde_json::from_str(&conditions).unwrap_or_default(),
        result: row.get(9)?,
        failure: row.get(10)?,
        confirmed: row.get::<_, i64>(11).unwrap_or(0) != 0,
    })
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64
}

/// Short, readable, and stable for the same input within a run.
fn short_id(prefix: &str, seed: &str) -> String {
    let mut value: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in seed.as_bytes() {
        value ^= *byte as u64;
        value = value.wrapping_mul(0x0000_0100_0000_01b3);
    }
    value ^= unix_now() as u64;
    format!("{}-{:08x}", prefix, (value & 0xffff_ffff) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> Graph {
        Graph::in_memory().expect("graph")
    }

    fn spec(title: &str, after: &[&str]) -> NodeSpec {
        NodeSpec {
            title: title.to_string(),
            detail: String::new(),
            after: after.iter().map(|s| s.to_string()).collect(),
            conditions: Vec::new(),
            retry_budget: 2,
        }
    }

    fn plan_three(graph: &Graph) -> String {
        let goal = graph.create_goal("tidy the downloads folder").expect("goal");
        graph
            .plan(
                &goal,
                &[
                    spec("list the folder", &[]),
                    spec("sort by type", &["list the folder"]),
                    spec("report what changed", &["sort by type"]),
                ],
            )
            .expect("plan");
        goal
    }

    #[test]
    fn a_goal_decomposes_into_nodes() {
        let graph = graph();
        let goal = plan_three(&graph);
        let nodes = graph.nodes(&goal).expect("nodes");
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].title, "list the folder");
    }

    #[test]
    fn only_the_first_node_is_eligible_at_the_start() {
        let graph = graph();
        plan_three(&graph);
        let eligible = graph.eligible(&SystemState::unrestricted()).expect("eligible");
        assert_eq!(eligible.len(), 1, "dependencies must hold the rest back");
        assert_eq!(eligible[0].title, "list the folder");
    }

    #[test]
    fn a_dependent_node_unblocks_when_its_dependency_finishes() {
        let graph = graph();
        let goal = plan_three(&graph);
        let first = graph.eligible(&SystemState::unrestricted()).expect("eligible")[0].clone();

        graph.start(&first.id).expect("start");
        graph.finish(&first.id, "12 files").expect("finish");

        let eligible = graph.eligible(&SystemState::unrestricted()).expect("eligible");
        assert_eq!(eligible.len(), 1);
        assert_eq!(eligible[0].title, "sort by type");
        let nodes = graph.nodes(&goal).expect("nodes");
        assert_eq!(nodes[0].state, "done");
        assert_eq!(nodes[0].result.as_deref(), Some("12 files"));
    }

    #[test]
    fn a_failure_retries_until_the_budget_is_spent() {
        let graph = graph();
        plan_three(&graph);
        let node = graph.eligible(&SystemState::unrestricted()).expect("eligible")[0].clone();

        // Budget of 2 means three attempts in total.
        for attempt in 1..=2 {
            graph.start(&node.id).expect("start");
            let state = graph.fail(&node.id, "the tool errored").expect("fail");
            assert_eq!(
                state,
                NodeState::Pending,
                "attempt {} should still retry",
                attempt
            );
        }

        graph.start(&node.id).expect("start");
        let state = graph.fail(&node.id, "again").expect("fail");
        assert_eq!(
            state,
            NodeState::Failed,
            "a spent budget must stop retrying and ask for a re-plan"
        );
        assert_eq!(graph.failed_nodes().expect("failed").len(), 1);
    }

    #[test]
    fn a_failed_node_marks_its_goal_for_replanning() {
        let graph = graph();
        let goal = plan_three(&graph);
        let node = graph.eligible(&SystemState::unrestricted()).expect("eligible")[0].clone();
        for _ in 0..3 {
            graph.start(&node.id).expect("start");
            let _ = graph.fail(&node.id, "no");
        }
        assert_eq!(graph.goal(&goal).expect("goal").state, "needs-replan");
    }

    #[test]
    fn a_goal_is_done_when_every_node_is() {
        let graph = graph();
        let goal = plan_three(&graph);
        for _ in 0..3 {
            let next = graph.eligible(&SystemState::unrestricted()).expect("eligible");
            let node = next.first().expect("a node").clone();
            graph.start(&node.id).expect("start");
            graph.finish(&node.id, "ok").expect("finish");
        }
        assert_eq!(graph.goal(&goal).expect("goal").state, "done");
        assert!(graph
            .eligible(&SystemState::unrestricted())
            .expect("eligible")
            .is_empty());
    }

    #[test]
    fn an_approval_lets_the_next_attempt_through() {
        // Without this, policy would prompt again on the next attempt and the
        // node would sit in needs-user forever, however many times it was
        // approved.
        let graph = graph();
        plan_three(&graph);
        let node = graph.eligible(&SystemState::unrestricted()).expect("eligible")[0].clone();
        graph.needs_user(&node.id, "cannot be undone").expect("needs user");

        graph.approve(&node.id).expect("approve");
        let approved = graph.node(&node.id).expect("node");
        assert_eq!(approved.state, "pending", "it must be runnable again");
        assert!(approved.confirmed, "the approval must be carried");

        graph.clear_confirmation(&node.id).expect("clear");
        assert!(
            !graph.node(&node.id).expect("node").confirmed,
            "one yes covers one attempt"
        );
    }

    #[test]
    fn a_node_awaiting_a_person_is_not_eligible() {
        let graph = graph();
        plan_three(&graph);
        let node = graph.eligible(&SystemState::unrestricted()).expect("eligible")[0].clone();
        graph.needs_user(&node.id, "deleting cannot be undone").expect("needs user");

        assert!(graph
            .eligible(&SystemState::unrestricted())
            .expect("eligible")
            .is_empty());
        assert_eq!(graph.node(&node.id).expect("node").state, "needs-user");
    }

    #[test]
    fn a_condition_holds_a_node_back_rather_than_failing_it() {
        let graph = graph();
        let goal = graph.create_goal("reindex everything").expect("goal");
        graph
            .plan(
                &goal,
                &[NodeSpec {
                    title: "reindex".to_string(),
                    detail: String::new(),
                    after: Vec::new(),
                    conditions: vec![Condition::OnAcPower],
                    retry_budget: 2,
                }],
            )
            .expect("plan");

        let on_battery = SystemState {
            on_ac_power: false,
            ..SystemState::unrestricted()
        };
        assert!(
            graph.eligible(&on_battery).expect("eligible").is_empty(),
            "a node wanting AC power must wait, not fail"
        );
        assert_eq!(
            graph.eligible(&SystemState::unrestricted()).expect("eligible").len(),
            1
        );
        // Waiting is not failing.
        assert_eq!(graph.nodes(&goal).expect("nodes")[0].state, "pending");
    }

    #[test]
    fn a_node_left_running_by_a_crash_comes_back_pending() {
        let graph = graph();
        plan_three(&graph);
        let node = graph.eligible(&SystemState::unrestricted()).expect("eligible")[0].clone();
        graph.start(&node.id).expect("start");
        assert_eq!(graph.node(&node.id).expect("node").state, "running");

        // What open() does on startup.
        let recovered = graph.recover().expect("recover");
        assert_eq!(recovered, 1);
        assert_eq!(
            graph.node(&node.id).expect("node").state,
            "pending",
            "a crash must not leave a node running forever"
        );
    }

    #[test]
    fn a_replan_keeps_finished_work() {
        let graph = graph();
        let goal = plan_three(&graph);
        let first = graph.eligible(&SystemState::unrestricted()).expect("eligible")[0].clone();
        graph.start(&first.id).expect("start");
        graph.finish(&first.id, "done already").expect("finish");

        graph
            .plan(&goal, &[spec("a different second step", &[])])
            .expect("replan");

        let nodes = graph.nodes(&goal).expect("nodes");
        assert_eq!(nodes.len(), 2, "the finished node must survive a re-plan");
        assert!(nodes.iter().any(|n| n.state == "done"));
        assert!(nodes.iter().any(|n| n.title == "a different second step"));
    }

    #[test]
    fn an_empty_plan_is_refused() {
        let graph = graph();
        let goal = graph.create_goal("do something").expect("goal");
        assert!(graph.plan(&goal, &[]).is_err());
    }

    #[test]
    fn listing_goals_shows_the_newest_first() {
        let graph = graph();
        graph.create_goal("first").expect("goal");
        graph.create_goal("second").expect("goal");
        let goals = graph.goals().expect("goals");
        assert_eq!(goals.len(), 2);
    }
}
