//! Filesystem tools, and the only path by which XOS touches a disk.
//!
//! Every call runs the same four steps in the same order, and the order is the
//! point:
//!
//! 1. The policy engine judges it. A blocked call stops here.
//! 2. A [`Permit`] is issued. Nothing runs without one.
//! 3. The journal snapshots what the action is about to destroy, *before* it
//!    happens, because afterwards the state it needed is gone.
//! 4. The action runs, and only then.
//!
//! Journalling never causes a failure: a snapshot that could not be taken tags
//! the entry unreversible and the work proceeds. Policy refusal does stop the
//! work, which is the difference between the two systems.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::journal::{Action, ActionKind, Journal};
use crate::policy::{Context as PolicyContext, Permit, Policy, PolicyDecision};

#[derive(Debug, Deserialize)]
pub struct ToolCall {
    pub tool: String,
    #[serde(default)]
    pub arguments: Value,
    /// The goal or node that asked, so the journal can be filtered by it.
    #[serde(default)]
    pub goal: Option<String>,
    /// A prompt decision the user has already given.
    #[serde(default)]
    pub confirmed: bool,
}

pub struct Outcome {
    pub decision: PolicyDecision,
    pub ran: bool,
    pub detail: String,
    pub journal_id: Option<i64>,
}

/// Run one filesystem tool, through policy and the journal.
pub fn run(
    call: &ToolCall,
    policy: &Arc<Policy>,
    journal: &Arc<Journal>,
    context: &PolicyContext,
) -> Outcome {
    // 1. Judge it.
    let decision = policy.evaluate(&call.tool, &call.arguments, context);
    if let PolicyDecision::Block { reason } = &decision {
        return Outcome {
            ran: false,
            detail: reason.clone(),
            decision: decision.clone(),
            journal_id: None,
        };
    }
    // An irreversible action waits for a person, unless one has already said yes.
    if let PolicyDecision::Prompt { reason } = &decision {
        if !call.confirmed {
            return Outcome {
                ran: false,
                detail: format!("{}. Confirm to go ahead.", reason),
                decision: decision.clone(),
                journal_id: None,
            };
        }
    }

    // 2. A permit, which is the only thing that lets a tool run.
    let Some(permit) = Permit::issue(&call.tool, decision.clone()) else {
        return Outcome {
            ran: false,
            detail: "the policy engine issued no permit".to_string(),
            decision,
            journal_id: None,
        };
    };
    if !permit.authorises(&call.tool) {
        return Outcome {
            ran: false,
            detail: "the permit does not cover this call".to_string(),
            decision,
            journal_id: None,
        };
    }

    let Some(action) = describe(call) else {
        return Outcome {
            ran: false,
            detail: format!("`{}` is not a filesystem tool", call.tool),
            decision,
            journal_id: None,
        };
    };

    // 3. Snapshot before acting.
    let journal_id = journal.snapshot(&action);

    // 4. Act.
    match perform(&action, &call.arguments) {
        Ok(detail) => Outcome {
            decision,
            ran: true,
            detail,
            journal_id: Some(journal_id),
        },
        Err(error) => Outcome {
            decision,
            ran: false,
            detail: error,
            journal_id: Some(journal_id),
        },
    }
}

/// Map a tool name and its arguments onto a journalled action.
fn describe(call: &ToolCall) -> Option<Action> {
    let path = call
        .arguments
        .get("path")
        .and_then(Value::as_str)
        .map(PathBuf::from)?;
    let goal = call.goal.clone();

    let action = match call.tool.as_str() {
        "write_file" => Action::new(&call.tool, ActionKind::Write, path),
        "delete_file" => Action::new(&call.tool, ActionKind::Delete, path),
        "make_directory" => Action::new(&call.tool, ActionKind::Mkdir, path),
        "chmod_file" => Action::new(&call.tool, ActionKind::Chmod, path),
        "move_file" => {
            let destination = call
                .arguments
                .get("destination")
                .and_then(Value::as_str)
                .map(PathBuf::from)?;
            Action::moving(&call.tool, path, destination)
        }
        _ => return None,
    };

    let mut action = Action {
        arguments: call.arguments.to_string(),
        ..action
    };
    if let Some(goal) = goal {
        action = action.for_goal(&goal);
    }
    Some(action)
}

fn perform(action: &Action, arguments: &Value) -> Result<String, String> {
    match action.kind {
        ActionKind::Write => {
            let content = arguments
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if let Some(parent) = action.path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(&action.path, content).map_err(|e| e.to_string())?;
            Ok(format!("wrote {}", action.path.display()))
        }
        ActionKind::Delete => {
            fs::remove_file(&action.path).map_err(|e| e.to_string())?;
            Ok(format!("deleted {}", action.path.display()))
        }
        ActionKind::Mkdir => {
            fs::create_dir_all(&action.path).map_err(|e| e.to_string())?;
            Ok(format!("created {}", action.path.display()))
        }
        ActionKind::Move => {
            let destination = action
                .destination
                .as_ref()
                .ok_or_else(|| "no destination".to_string())?;
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::rename(&action.path, destination).map_err(|e| e.to_string())?;
            Ok(format!(
                "moved {} to {}",
                action.path.display(),
                destination.display()
            ))
        }
        ActionKind::Chmod => {
            let mode = arguments
                .get("mode")
                .and_then(Value::as_u64)
                .ok_or_else(|| "no mode given".to_string())?;
            set_mode(&action.path, mode as u32)?;
            Ok(format!("set the mode of {}", action.path.display()))
        }
        ActionKind::Unreversible => Err("not a filesystem action".to_string()),
    }
}

#[cfg(unix)]
fn set_mode(path: &std::path::Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn set_mode(_path: &std::path::Path, _mode: u32) -> Result<(), String> {
    Err("changing a mode needs a Unix system".to_string())
}

pub fn outcome_json(outcome: &Outcome) -> Value {
    json!({
        "decision": outcome.decision.label(),
        "reason": outcome.decision.reason(),
        "ran": outcome.ran,
        "detail": outcome.detail,
        "journal_id": outcome.journal_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PolicyConfig;

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        policy: Arc<Policy>,
        journal: Arc<Journal>,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("work");
        fs::create_dir_all(&root).expect("work");
        Fixture {
            policy: Arc::new(Policy::new(PolicyConfig::default())),
            journal: Arc::new(
                Journal::in_memory(dir.path().join("store")).expect("journal"),
            ),
            root,
            _dir: dir,
        }
    }

    fn call(tool: &str, arguments: Value) -> ToolCall {
        ToolCall {
            tool: tool.to_string(),
            arguments,
            goal: None,
            confirmed: false,
        }
    }

    #[test]
    fn a_write_runs_and_is_journalled() {
        let f = fixture();
        let path = f.root.join("a.txt");
        let outcome = run(
            &call("write_file", json!({"path": path, "content": "hello"})),
            &f.policy,
            &f.journal,
            &PolicyContext::default(),
        );
        assert!(outcome.ran, "{}", outcome.detail);
        assert_eq!(fs::read_to_string(&path).expect("read"), "hello");
        assert!(outcome.journal_id.unwrap_or(-1) > 0, "it must be journalled");
    }

    #[test]
    fn a_blocked_path_never_touches_the_disk() {
        let f = fixture();
        let outcome = run(
            &call("write_file", json!({"path": "/home/me/.ssh/authorized_keys", "content": "x"})),
            &f.policy,
            &f.journal,
            &PolicyContext::default(),
        );
        assert!(!outcome.ran);
        assert_eq!(outcome.decision.label(), "block");
        assert!(!std::path::Path::new("/home/me/.ssh/authorized_keys").exists());
    }

    #[test]
    fn an_irreversible_action_waits_for_a_person() {
        let f = fixture();
        let path = f.root.join("gone.txt");
        fs::write(&path, "still here").expect("write");

        let outcome = run(
            &call("delete_file", json!({"path": path})),
            &f.policy,
            &f.journal,
            &PolicyContext::default(),
        );
        assert!(!outcome.ran, "a delete must not run unconfirmed");
        assert_eq!(outcome.decision.label(), "prompt");
        assert!(path.exists(), "the file must still be there");
    }

    #[test]
    fn a_confirmed_irreversible_action_runs_and_can_be_undone() {
        let f = fixture();
        let path = f.root.join("gone.txt");
        fs::write(&path, "important").expect("write");

        let mut request = call("delete_file", json!({"path": path.clone()}));
        request.confirmed = true;
        let outcome = run(&request, &f.policy, &f.journal, &PolicyContext::default());
        assert!(outcome.ran, "{}", outcome.detail);
        assert!(!path.exists());

        f.journal.undo(1).expect("undo");
        assert_eq!(fs::read_to_string(&path).expect("read"), "important");
    }

    #[test]
    fn a_reorganisation_can_be_put_back() {
        let f = fixture();
        let one = f.root.join("one.txt");
        fs::write(&one, "first").expect("write");
        let moved = f.root.join("sorted").join("one.txt");

        let mut request = call(
            "move_file",
            json!({"path": one.clone(), "destination": moved.clone()}),
        );
        request.confirmed = true;
        let outcome = run(&request, &f.policy, &f.journal, &PolicyContext::default());
        assert!(outcome.ran, "{}", outcome.detail);
        assert!(moved.exists() && !one.exists());

        f.journal.undo(1).expect("undo");
        assert!(one.exists() && !moved.exists());
    }

    #[test]
    fn an_unknown_tool_does_nothing() {
        let f = fixture();
        let outcome = run(
            &call("launch_rocket", json!({"path": "/tmp/x"})),
            &f.policy,
            &f.journal,
            &PolicyContext::default(),
        );
        assert!(!outcome.ran);
        assert!(outcome.detail.contains("not a filesystem tool"));
    }
}
