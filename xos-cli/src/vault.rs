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
