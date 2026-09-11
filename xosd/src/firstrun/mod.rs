//! The first goal.
//!
//! When the wizard finishes, XOS does not drop someone at an empty prompt. It
//! runs one goal, by itself, while they watch.
//!
//! The goal is built in and fixed: take stock of the machine, look at the
//! obvious project directories, and write down what this person appears to work
//! on. It is chosen for three reasons, in this order.
//!
//! It is zero-risk. Nothing is written outside memory, nothing leaves the
//! machine, and nothing is irreversible. Somebody who has just installed an
//! operating system that wants to act on their behalf is owed a first
//! demonstration they cannot regret.
//!
//! It shows the whole loop. A goal is declared, decomposed into nodes, the nodes
//! run, memory is written, and all of it appears in Mission Control as it
//! happens. That is the entire architecture, visible once, before anyone is
//! asked to trust it with something real.
//!
//! And it seeds memory, so the second thing anyone says to XOS is already
//! answered by a system that knows a little about them.
//!
//! # Read-only, and quiet about what it cannot read
//!
//! Every directory here is listed, never opened. A missing or unreadable
//! directory is skipped without a word: somebody whose home does not look like
//! the assumed shape should not have their first minute with XOS be a list of
//! complaints about it.

pub mod scan;

use std::sync::Arc;

use serde::Serialize;
use serde_json::json;

use crate::graph::{NodeSpec, NodeState};
use crate::memory::Tier;
use crate::rpc::Daemon;

pub use scan::Scan;

/// The goal, worded as it appears in Mission Control.
pub const DESCRIPTION: &str = "Take stock of this machine and what I work on";

/// Whether the first goal has run on this machine.
pub fn has_run(daemon: &Arc<Daemon>) -> bool {
    daemon
        .graph
        .goals()
        .map(|goals| goals.iter().any(|goal| goal.description == DESCRIPTION))
        .unwrap_or(false)
}

#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub goal_id: String,
    pub steps: Vec<Step>,
    pub summary: String,
    pub memory_id: Option<i64>,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub title: String,
    pub state: String,
    pub result: String,
}

/// The fixed decomposition.
///
/// Written out rather than asked for. A model is capable of planning this, and
/// having it do so would mean the first thing anyone sees is non-deterministic,
/// possibly wrong, and impossible to describe in advance. The point of this goal
/// is that it does exactly what it says it will.
fn plan() -> Vec<NodeSpec> {
    vec![
        NodeSpec {
            title: "Take stock of this machine".to_string(),
            detail: "Read the hardware inventory: what is here, what drives it, \
                     and what XOS can run on it."
                .to_string(),
            after: Vec::new(),
            conditions: Vec::new(),
            retry_budget: 1,
        },
        NodeSpec {
            title: "Look at the obvious project directories".to_string(),
            detail: "List ~/Documents, ~/Projects, ~/code and ~/Downloads. \
                     Names only, nothing opened. Anything missing is skipped."
                .to_string(),
            after: vec!["Take stock of this machine".to_string()],
            conditions: Vec::new(),
            retry_budget: 1,
        },
        NodeSpec {
            title: "Write down what I appear to work on".to_string(),
            detail: "Put a short summary into long-term memory, so the next \
                     conversation starts from something."
                .to_string(),
            after: vec!["Look at the obvious project directories".to_string()],
            conditions: Vec::new(),
            retry_budget: 1,
        },
    ]
}

/// Run it.
///
/// Each node is moved through the graph as it happens rather than after the
/// fact, so somebody watching Mission Control sees it work rather than seeing
/// three nodes turn green at once.
pub fn run(daemon: &Arc<Daemon>) -> Result<Outcome, String> {
    let goal_id = daemon.graph.create_goal(DESCRIPTION)?;
    daemon.graph.plan(&goal_id, &plan())?;

    let nodes = daemon.graph.nodes(&goal_id)?;
    let mut steps = Vec::new();
    let mut scan = Scan::default();

    for (index, node) in nodes.iter().enumerate() {
        let _ = daemon.graph.start(&node.id);

        let result = match index {
            0 => {
                let inventory = crate::hardware::Inventory::read();
                scan.machine = inventory.summary();
                scan.profile = inventory.profile().label().to_string();
                format!("{} ({})", scan.machine, scan.profile)
            }
            1 => {
                // The only place this goal touches the filesystem, and it only
                // ever lists.
                scan.findings = scan::look(&daemon.policy, &mut scan.skipped);
                if scan.findings.is_empty() {
                    "Nothing in the usual places, which is not a problem".to_string()
                } else {
                    scan.findings
                        .iter()
                        .map(|finding| {
                            format!("{} ({} items)", finding.directory, finding.entries)
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            }
            _ => {
                scan.summary = scan.compose();
                match daemon.memory.write(
                    Tier::LongTerm,
                    &scan.summary,
                    "first-run,machine,work",
                    "the first goal",
                ) {
                    Ok(id) => {
                        scan.memory_id = Some(id);
                        format!("Written to long-term memory as entry {}", id)
                    }
                    Err(error) => format!("Could not write to memory: {}", error),
                }
            }
        };

        let _ = daemon.graph.finish(&node.id, &result);
        steps.push(Step {
            title: node.title.clone(),
            state: NodeState::Done.label().to_string(),
            result,
        });
    }

    Ok(Outcome {
        goal_id,
        steps,
        summary: scan.summary,
        memory_id: scan.memory_id,
        skipped: scan.skipped,
    })
}

/// What the wizard shows about the first goal before running it, so nobody is
/// surprised by what it does.
pub fn describe() -> serde_json::Value {
    json!({
        "description": DESCRIPTION,
        "steps": plan().iter().map(|spec| json!({
            "title": spec.title,
            "detail": spec.detail,
        })).collect::<Vec<_>>(),
        "reads": scan::DIRECTORIES,
        "writes": "one entry in long-term memory, and nothing else",
        "network": false,
        "reversible": true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_is_fixed_rather_than_asked_for() {
        // The first thing anyone sees must do exactly what it said it would.
        assert_eq!(plan().len(), 3);
        assert_eq!(plan()[0].title, "Take stock of this machine");
        // And each step waits for the one before, so Mission Control shows it
        // working rather than finishing.
        assert!(plan()[1].after.contains(&plan()[0].title));
        assert!(plan()[2].after.contains(&plan()[1].title));
    }

    #[test]
    fn what_it_will_do_is_stated_before_it_does_it() {
        let description = describe();
        assert_eq!(description["network"], false);
        assert_eq!(description["reversible"], true);
        assert!(description["writes"]
            .as_str()
            .expect("writes")
            .contains("long-term memory"));
        assert_eq!(
            description["steps"].as_array().expect("steps").len(),
            3,
            "every step is named in advance"
        );
    }

    #[test]
    fn the_description_is_stable_so_it_is_only_ever_run_once() {
        // `has_run` matches on this exact string. Changing it would run the
        // first goal again on a machine that has had it.
        assert_eq!(DESCRIPTION, "Take stock of this machine and what I work on");
    }
}
