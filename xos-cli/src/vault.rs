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
