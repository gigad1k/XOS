//! `xos vault` and `xos spend`.
//!
//! The key is typed here and sent straight to the daemon, which is the only
//! process that stores or reads it. It is never echoed, never written to a
//! file by the client, and never comes back: `vault list` returns names.

use std::io::{BufRead, IsTerminal};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use serde_json::{json, Value};

use crate::socket::Connection;

pub fn add(connection: &mut Connection, provider: &str) -> Result<String, String> {
    let key = read_secret(&format!("Key for {}: ", provider))?;
    if key.trim().is_empty() {
        return Err("no key was entered, nothing was stored".to_string());
    }

    connection.call(
        "vault.set",
        json!({"provider": provider, "key": key.trim()}),
    )?;
    // The key is not echoed back here or anywhere else.
    Ok(format!(
        "Stored a credential for {}. Run `xos vault list` to see what is held.\n",
        provider
    ))
}

pub fn list(connection: &mut Connection) -> Result<String, String> {
    let result = connection.call("vault.list", json!({}))?;
    let store = result
        .get("store")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let providers = result
        .get("providers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "store     {}", store);
    let _ = writeln!(out);
    if providers.is_empty() {
        let _ = writeln!(
            out,
            "No credentials are stored. Add one with `xos vault add <provider>`."
        );
        return Ok(out);
    }
    for provider in providers {
        if let Some(name) = provider.as_str() {
            let _ = writeln!(out, "{}", name);
        }
    }
    Ok(out)
}

pub fn remove(connection: &mut Connection, provider: &str) -> Result<String, String> {
    connection.call("vault.remove", json!({"provider": provider}))?;
    Ok(format!("Removed the credential for {}.\n", provider))
}

pub fn spend(connection: &mut Connection, days: u32) -> Result<String, String> {
    let result = connection.call("spend.report", json!({"days": days}))?;
    let rows = result
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let today = result
        .get("today_total")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);

    use std::fmt::Write as _;
    let mut out = String::new();
    if rows.is_empty() {
        let _ = writeln!(out, "No spend recorded in the last {} days.", days);
        return Ok(out);
    }

    let _ = writeln!(
        out,
        "{:<12} {:<16} {:>10} {:>10} {:>10}",
        "day", "provider", "tokens in", "out", "cost"
    );
    for row in &rows {
        let _ = writeln!(
            out,
            "{:<12} {:<16} {:>10} {:>10} {:>10.4}",
            row.get("day").and_then(Value::as_str).unwrap_or("-"),
            row.get("provider").and_then(Value::as_str).unwrap_or("-"),
            row.get("tokens_in").and_then(Value::as_i64).unwrap_or(0),
            row.get("tokens_out").and_then(Value::as_i64).unwrap_or(0),
            row.get("cost").and_then(Value::as_f64).unwrap_or(0.0),
        );
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "today {:.4}", today);
    if let Some(cap) = result.pointer("/caps/daily_total").and_then(Value::as_f64) {
        let _ = writeln!(out, "cap   {:.4}", cap);
        if today >= cap {
            let _ = writeln!(
                out,
                "The global cap is spent. Cloud providers stay unavailable until tomorrow."
            );
        }
    }
    Ok(out)
}

/// Read a secret without echoing it.
///
/// When stdin is not a terminal the key is read from it directly, so
/// `xos vault add openrouter < key.txt` works in a script.
fn read_secret(prompt: &str) -> Result<String, String> {
    if !std::io::stdin().is_terminal() {
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| format!("cannot read the key: {}", e))?;
        return Ok(line.trim_end_matches(['\r', '\n']).to_string());
    }

    eprint!("{}", prompt);
    enable_raw_mode().map_err(|e| format!("cannot hide the key as you type: {}", e))?;
    let mut key = String::new();
    let outcome = loop {
        match event::read() {
            Ok(Event::Key(press)) if press.kind == KeyEventKind::Press => {
                match press.code {
                    KeyCode::Enter => break Ok(()),
                    KeyCode::Backspace => {
                        key.pop();
                    }
                    KeyCode::Esc => break Err("cancelled".to_string()),
                    KeyCode::Char('c') if press.modifiers.contains(KeyModifiers::CONTROL) => {
                        break Err("cancelled".to_string())
                    }
                    KeyCode::Char(c) => key.push(c),
                    _ => {}
                }
            }
            Ok(_) => {}
            Err(error) => break Err(format!("cannot read the key: {}", error)),
        }
    };
    let _ = disable_raw_mode();
    eprintln!();

    match outcome {
        Ok(()) => Ok(key),
        Err(error) => Err(error),
    }
}

/// `xos escalations` — what left the machine, and why.
pub fn escalations(connection: &mut Connection, limit: u32) -> Result<String, String> {
    let result = connection.call("escalations.list", json!({"limit": limit}))?;
    let entries = result
        .get("entries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    use std::fmt::Write as _;
    let mut out = String::new();
    if entries.is_empty() {
        let _ = writeln!(out, "No escalations recorded. Everything has stayed local.");
        return Ok(out);
    }

    let _ = writeln!(
        out,
        "{:<18} {:<16} {:<10} {:>7} {:>7} {:>9}  {}",
        "trigger", "target", "class", "in", "out", "cost", "reason"
    );
    for entry in &entries {
        let text = |key: &str| {
            entry
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("-")
                .to_string()
        };
        let number = |key: &str| entry.get(key).and_then(Value::as_i64).unwrap_or(0);
        let _ = writeln!(
            out,
            "{:<18} {:<16} {:<10} {:>7} {:>7} {:>9.4}  {}",
            text("trigger"),
            text("target_model"),
            text("task_class"),
            number("tokens_in"),
            number("tokens_out"),
            entry.get("cost").and_then(Value::as_f64).unwrap_or(0.0),
            text("reason"),
        );
    }
    Ok(out)
}

/// `xos mode` — read or set the cost mode.
pub fn mode(connection: &mut Connection, wanted: Option<&str>) -> Result<String, String> {
    let result = match wanted {
        Some(mode) => connection.call("mode.set", json!({"mode": mode}))?,
        None => connection.call("mode.get", json!({}))?,
    };
    let mode = result.get("mode").and_then(Value::as_str).unwrap_or("unknown");
    Ok(match wanted {
        Some(_) => format!("Cost mode set to {}.\n", mode),
        None => format!("{}\n", mode),
    })
}

/// `xos memory search`
pub fn memory_search(
    connection: &mut Connection,
    query: &str,
    tier: Option<&str>,
    limit: u32,
) -> Result<String, String> {
    let mut params = json!({"query": query, "limit": limit});
    if let Some(tier) = tier {
        params["tier"] = json!(tier);
    }
    let result = connection.call("memory.recall", params)?;
    let hits = result
        .get("hits")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    use std::fmt::Write as _;
    let mut out = String::new();
    if hits.is_empty() {
        let _ = writeln!(out, "Nothing matches `{}` yet.", query);
        return Ok(out);
    }
    let _ = writeln!(out, "{:<10} {:>6}  {}", "tier", "score", "content");
    for hit in &hits {
        let content = hit.get("content").and_then(Value::as_str).unwrap_or("");
        let one_line: String = content.split('\n').collect::<Vec<_>>().join(" ");
        let _ = writeln!(
            out,
            "{:<10} {:>6.2}  {}",
            hit.get("tier").and_then(Value::as_str).unwrap_or("-"),
            hit.get("score").and_then(Value::as_f64).unwrap_or(0.0),
            one_line
        );
    }
    Ok(out)
}

/// `xos memory stats`
pub fn memory_stats(connection: &mut Connection) -> Result<String, String> {
    let result = connection.call("memory.stats", json!({}))?;
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "embedder  {}",
        result.get("embedder").and_then(Value::as_str).unwrap_or("-")
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "{:<12} {:>8}", "tier", "entries");
    for tier in result
        .get("tiers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let _ = writeln!(
            out,
            "{:<12} {:>8}",
            tier.get("tier").and_then(Value::as_str).unwrap_or("-"),
            tier.get("entries").and_then(Value::as_i64).unwrap_or(0)
        );
    }
    Ok(out)
}

/// `xos memory write`
pub fn memory_write(
    connection: &mut Connection,
    tier: &str,
    content: &str,
    tags: &str,
) -> Result<String, String> {
    connection.call(
        "memory.write",
        json!({"tier": tier, "content": content, "tags": tags, "source": "cli"}),
    )?;
    Ok(format!("Remembered, in {} memory.\n", tier))
}

/// `xos memory promote` — close a task or a day.
pub fn memory_promote(
    connection: &mut Connection,
    from: &str,
    to: Option<&str>,
) -> Result<String, String> {
    let mut params = json!({"from": from});
    if let Some(to) = to {
        params["to"] = json!(to);
    }
    let result = connection.call("memory.promote", params)?;
    if !result
        .get("promoted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let reason = result
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("nothing to promote");
        return Ok(format!("{}\n", reason));
    }

    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Summarised {} {} entries into {}.",
        result.get("summarised").and_then(Value::as_i64).unwrap_or(0),
        result.get("from").and_then(Value::as_str).unwrap_or("-"),
        result.get("to").and_then(Value::as_str).unwrap_or("-")
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{}",
        result.get("summary").and_then(Value::as_str).unwrap_or("")
    );
    Ok(out)
}

/// `xos export`
pub fn export(connection: &mut Connection, path: &str) -> Result<String, String> {
    let passphrase = read_secret("Passphrase for the bundle: ")?;
    if passphrase.trim().is_empty() {
        return Err("a bundle needs a passphrase; it holds your memory".to_string());
    }
    let result = connection.call(
        "memory.export",
        json!({"path": path, "passphrase": passphrase}),
    )?;
    Ok(format!(
        "Wrote {} memory entries to {}. Keep the passphrase: without it the bundle is scrap.\n",
        result.get("entries").and_then(Value::as_i64).unwrap_or(0),
        path
    ))
}

/// `xos import`
pub fn import(connection: &mut Connection, path: &str) -> Result<String, String> {
    let passphrase = read_secret("Passphrase for the bundle: ")?;
    let result = connection.call(
        "memory.import",
        json!({"path": path, "passphrase": passphrase}),
    )?;

    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Merged {} new entries, kept {} this machine already had.",
        result.get("added").and_then(Value::as_i64).unwrap_or(0),
        result.get("kept").and_then(Value::as_i64).unwrap_or(0)
    );
    let providers = result
        .get("providers_to_reconnect")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !providers.is_empty() {
        let names: Vec<&str> = providers.iter().filter_map(Value::as_str).collect();
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "The bundle records credentials for: {}. Keys are never exported, so add them again with `xos vault add <provider>`.",
            names.join(", ")
        );
    }
    Ok(out)
}

/// `xos policy test <tool> <args>`
pub fn policy_test(
    connection: &mut Connection,
    tool: &str,
    arguments: &str,
) -> Result<String, String> {
    let parsed: Value = if arguments.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(arguments)
            .map_err(|e| format!("the arguments are not JSON: {}", e))?
    };
    let result = connection.call("policy.test", json!({"tool": tool, "arguments": parsed}))?;

    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<10} {}",
        result.get("decision").and_then(Value::as_str).unwrap_or("-"),
        result.get("reason").and_then(Value::as_str).unwrap_or("")
    );
    if !result.get("runs").and_then(Value::as_bool).unwrap_or(false) {
        let _ = writeln!(out, "This call does not run, whatever you answer next.");
    }
    Ok(out)
}

/// `xos policy log`
pub fn policy_log(connection: &mut Connection, limit: u32) -> Result<String, String> {
    let result = connection.call("policy.log", json!({"limit": limit}))?;
    let entries = result
        .get("entries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "strictness  {}",
        result.get("strictness").and_then(Value::as_str).unwrap_or("-")
    );
    let _ = writeln!(out);
    if entries.is_empty() {
        let _ = writeln!(out, "No decisions recorded yet.");
        return Ok(out);
    }
    let _ = writeln!(
        out,
        "{:<9} {:<20} {:<10}  {}",
        "decision", "tool", "source", "reason"
    );
    for entry in &entries {
        let text = |key: &str| entry.get(key).and_then(Value::as_str).unwrap_or("-");
        let _ = writeln!(
            out,
            "{:<9} {:<20} {:<10}  {}",
            text("decision"),
            text("tool"),
            text("source"),
            text("reason")
        );
    }
    Ok(out)
}

/// `xos journal`
pub fn journal(
    connection: &mut Connection,
    goal: Option<&str>,
    tool: Option<&str>,
    limit: u32,
) -> Result<String, String> {
    let mut params = json!({"limit": limit});
    if let Some(goal) = goal {
        params["goal"] = json!(goal);
    }
    if let Some(tool) = tool {
        params["tool"] = json!(tool);
    }
    let result = connection.call("journal.list", params)?;
    let entries = result
        .get("entries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    use std::fmt::Write as _;
    let mut out = String::new();
    if entries.is_empty() {
        let _ = writeln!(out, "The journal is empty. Nothing has been done to undo.");
        return Ok(out);
    }
    let _ = writeln!(
        out,
        "{:<8} {:<16} {:<10} {:<12}  {}",
        "state", "tool", "kind", "goal", "path"
    );
    for entry in &entries {
        let text = |key: &str| entry.get(key).and_then(Value::as_str).unwrap_or("-");
        let state = if entry.get("undone").and_then(Value::as_bool).unwrap_or(false) {
            "undone"
        } else if entry.get("reversible").and_then(Value::as_bool).unwrap_or(false) {
            "undoable"
        } else {
            "final"
        };
        let _ = writeln!(
            out,
            "{:<8} {:<16} {:<10} {:<12}  {}",
            state,
            text("tool"),
            text("kind"),
            entry.get("goal").and_then(Value::as_str).unwrap_or("-"),
            text("path")
        );
    }
    Ok(out)
}

/// `xos undo`
pub fn undo(connection: &mut Connection, last: u32) -> Result<String, String> {
    let result = connection.call("journal.undo", json!({"last": last}))?;

    use std::fmt::Write as _;
    let mut out = String::new();
    let descriptions = result
        .get("descriptions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for description in &descriptions {
        if let Some(text) = description.as_str() {
            let _ = writeln!(out, "{}", text);
        }
    }
    if let Some(refused) = result.get("refused").and_then(Value::as_str) {
        if !descriptions.is_empty() {
            let _ = writeln!(out);
        }
        let _ = writeln!(out, "{}", refused);
        return Ok(out);
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "Undone {} actions.", descriptions.len());
    Ok(out)
}

/// `xos supervisor prompts`
pub fn supervisor_prompts(connection: &mut Connection) -> Result<String, String> {
    let result = connection.call("supervisor.prompts", json!({}))?;
    let prompts = result
        .get("prompts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "tokens today  {} of {}",
        result.get("tokens_today").and_then(Value::as_u64).unwrap_or(0),
        result.get("daily_ceiling").and_then(Value::as_u64).unwrap_or(0)
    );
    let _ = writeln!(out);
    if prompts.is_empty() {
        let _ = writeln!(out, "No prompts have been compiled yet.");
        return Ok(out);
    }
    let _ = writeln!(
        out,
        "{:<18} {:>4} {:>6} {:>6} {:>7} {:>8}  {}",
        "task type", "ver", "hits", "fails", "score", "guarded", "cache key"
    );
    for prompt in &prompts {
        let _ = writeln!(
            out,
            "{:<18} {:>4} {:>6} {:>6} {:>7} {:>8}  {}",
            prompt.get("task_type").and_then(Value::as_str).unwrap_or("-"),
            prompt.get("version").and_then(Value::as_i64).unwrap_or(0),
            prompt.get("hits").and_then(Value::as_i64).unwrap_or(0),
            prompt.get("failures").and_then(Value::as_i64).unwrap_or(0),
            prompt
                .get("score")
                .and_then(Value::as_f64)
                .map(|s| format!("{:.2}", s))
                .unwrap_or_else(|| "-".to_string()),
            if prompt.get("guarded").and_then(Value::as_bool).unwrap_or(false) {
                "yes"
            } else {
                "no"
            },
            prompt.get("cache_key").and_then(Value::as_str).unwrap_or("-")
        );
    }
    Ok(out)
}

/// `xos supervisor log`
pub fn supervisor_log(connection: &mut Connection, limit: u32) -> Result<String, String> {
    let result = connection.call("supervisor.log", json!({"limit": limit}))?;
    let entries = result
        .get("entries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let queued = result
        .get("queued")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    use std::fmt::Write as _;
    let mut out = String::new();
    if !queued.is_empty() {
        let _ = writeln!(
            out,
            "{} wakes are waiting for connectivity.",
            queued.len()
        );
        let _ = writeln!(out);
    }
    if entries.is_empty() {
        let _ = writeln!(out, "The supervisor has not considered a prompt yet.");
        return Ok(out);
    }
    let _ = writeln!(
        out,
        "{:<16} {:<14} {:>6} {:>6}  {}",
        "task type", "outcome", "was", "now", "note"
    );
    for entry in &entries {
        let score = |key: &str| {
            entry
                .get(key)
                .and_then(Value::as_f64)
                .map(|s| format!("{:.2}", s))
                .unwrap_or_else(|| "-".to_string())
        };
        let _ = writeln!(
            out,
            "{:<16} {:<14} {:>6} {:>6}  {}",
            entry.get("task_type").and_then(Value::as_str).unwrap_or("-"),
            entry.get("outcome").and_then(Value::as_str).unwrap_or("-"),
            score("old_score"),
            score("new_score"),
            entry.get("note").and_then(Value::as_str).unwrap_or("")
        );
    }
    Ok(out)
}

/// `xos goal new`
pub fn goal_new(connection: &mut Connection, description: &str) -> Result<String, String> {
    let result = connection.call("graph.create_goal", json!({"description": description}))?;
    let goal_id = result.get("goal_id").and_then(Value::as_str).unwrap_or("-");

    use std::fmt::Write as _;
    let mut out = String::new();
    if result.get("planned").and_then(Value::as_bool).unwrap_or(false)
        || result.get("nodes").is_some()
    {
        let _ = writeln!(
            out,
            "Goal {} created with {} steps. Run `xos goal show {}` to see them.",
            goal_id,
            result.get("nodes").and_then(Value::as_i64).unwrap_or(0),
            goal_id
        );
    } else {
        let _ = writeln!(out, "Goal {} created, but not yet planned.", goal_id);
        if let Some(reason) = result.get("reason").and_then(Value::as_str) {
            let _ = writeln!(out, "{}", reason);
        }
        if let Some(supervisor) = result.get("supervisor") {
            if let Some(reason) = supervisor.get("reason").and_then(Value::as_str) {
                let _ = writeln!(out, "{}", reason);
            }
        }
    }
    Ok(out)
}

/// `xos goal list`
pub fn goal_list(connection: &mut Connection) -> Result<String, String> {
    let result = connection.call("graph.list", json!({}))?;
    let goals = result
        .get("goals")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    use std::fmt::Write as _;
    let mut out = String::new();
    if goals.is_empty() {
        let _ = writeln!(out, "No goals yet. Start one with `xos goal new \"...\"`.");
        return Ok(out);
    }
    let _ = writeln!(out, "{:<18} {:<14}  {}", "id", "state", "goal");
    for goal in &goals {
        let _ = writeln!(
            out,
            "{:<18} {:<14}  {}",
            goal.get("id").and_then(Value::as_str).unwrap_or("-"),
            goal.get("state").and_then(Value::as_str).unwrap_or("-"),
            goal.get("description").and_then(Value::as_str).unwrap_or("")
        );
    }
    Ok(out)
}

/// `xos goal show`
pub fn goal_show(connection: &mut Connection, goal_id: &str) -> Result<String, String> {
    let result = connection.call("graph.status", json!({"goal_id": goal_id}))?;
    let goal = result.get("goal").cloned().unwrap_or(Value::Null);
    let nodes = result
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}  {}",
        goal.get("id").and_then(Value::as_str).unwrap_or("-"),
        goal.get("description").and_then(Value::as_str).unwrap_or("")
    );
    let _ = writeln!(
        out,
        "state  {}",
        goal.get("state").and_then(Value::as_str).unwrap_or("-")
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "{:<12} {:>8}  {}", "state", "attempts", "step");
    for node in &nodes {
        let _ = writeln!(
            out,
            "{:<12} {:>8}  {}",
            node.get("state").and_then(Value::as_str).unwrap_or("-"),
            node.get("attempts").and_then(Value::as_i64).unwrap_or(0),
            node.get("title").and_then(Value::as_str).unwrap_or("")
        );
        if let Some(result) = node.get("result").and_then(Value::as_str) {
            let one_line: String = result.split('\n').collect::<Vec<_>>().join(" ");
            let _ = writeln!(out, "{:<12} {:>8}  {}", "", "", one_line.chars().take(100).collect::<String>());
        }
    }
    Ok(out)
}

/// `xos goal advance`
pub fn goal_advance(connection: &mut Connection, limit: u32) -> Result<String, String> {
    let result = connection.call("graph.advance", json!({"limit": limit}))?;
    let nodes = result
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    use std::fmt::Write as _;
    let mut out = String::new();
    if nodes.is_empty() {
        let _ = writeln!(
            out,
            "{}",
            result
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("nothing ran")
        );
        return Ok(out);
    }
    for node in &nodes {
        let _ = writeln!(
            out,
            "{:<12}  {}",
            node.get("outcome").and_then(Value::as_str).unwrap_or("-"),
            node.get("node").and_then(Value::as_str).unwrap_or("")
        );
    }
    Ok(out)
}
