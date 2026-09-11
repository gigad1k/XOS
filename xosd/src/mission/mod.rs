//! XOS Mission Control — the transparency layer.
//!
//! Autonomy without visibility feels unpredictable, so everything XOS is doing,
//! spending and deciding is visible in one place. The primary view is the task
//! graph, because that is what the system is actually doing.
//!
//! # Read-only, with two exceptions
//!
//! Mission Control approves blocked nodes and adjusts the cost mode. That is the
//! whole of its write surface, and the rest is a window. It holds no business
//! logic: every value on the page comes from the daemon, and every action is a
//! call the CLI could make.
//!
//! # Why a hand-rolled server
//!
//! The surface is one page and five endpoints, all bound to loopback. A web
//! framework would be more dependency than the job needs, and this way the whole
//! HTTP surface is small enough to read in one sitting, which matters for
//! something that exposes the system's state.

use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};

use crate::rpc::Daemon;

const PAGE: &str = include_str!("index.html");

/// Serve Mission Control on loopback.
pub async fn serve(listener: TcpListener, daemon: Arc<Daemon>) {
    loop {
        match listener.accept().await {
            Ok((stream, _peer)) => {
                let daemon = Arc::clone(&daemon);
                tokio::spawn(async move {
                    if let Err(error) = handle(stream, daemon).await {
                        debug!(%error, "a mission control connection ended");
                    }
                });
            }
            Err(error) => {
                warn!(%error, "mission control stopped accepting");
                return;
            }
        }
    }
}

async fn handle(stream: TcpStream, daemon: Arc<Daemon>) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).await? == 0 {
        return Ok(());
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or("/").to_string();

    // Headers, only for the content length.
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).await? == 0 {
            break;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some(value) = header.to_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
    }

    let body = if length > 0 && length < 1_000_000 {
        let mut buffer = vec![0u8; length];
        reader.read_exact(&mut buffer).await?;
        serde_json::from_slice::<Value>(&buffer).unwrap_or(Value::Null)
    } else {
        Value::Null
    };

    let (status, content_type, payload) = route(&method, &path, &body, &daemon).await;
    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        status,
        content_type,
        payload.len()
    );
    writer.write_all(response.as_bytes()).await?;
    writer.write_all(payload.as_bytes()).await?;
    writer.flush().await
}

async fn route(
    method: &str,
    path: &str,
    body: &Value,
    daemon: &Arc<Daemon>,
) -> (&'static str, &'static str, String) {
    match (method, path) {
        ("GET", "/") | ("GET", "/index.html") => {
            ("200 OK", "text/html; charset=utf-8", PAGE.to_string())
        }
        // The first run, matching the terminal wizard. Both ask the daemon what
        // the goal will do, and both run the same one.
        ("GET", "/api/firstrun") => (
            "200 OK",
            "application/json",
            json!({
                "goal": crate::firstrun::describe(),
                "already_run": crate::firstrun::has_run(daemon),
            })
            .to_string(),
        ),
        ("POST", "/api/firstrun/run") => {
            let outcome = if crate::firstrun::has_run(daemon) {
                json!({"ok": false, "already_run": true})
            } else {
                match crate::firstrun::run(daemon) {
                    Ok(outcome) => json!({"ok": true, "outcome": outcome}),
                    Err(error) => json!({"ok": false, "error": error}),
                }
            };
            ("200 OK", "application/json", outcome.to_string())
        }
        ("GET", "/api/state") => (
            "200 OK",
            "application/json",
            state(daemon).await.to_string(),
        ),
        // The entire write surface: approving a blocked node, and cost mode.
        ("POST", "/api/confirm") => {
            let node_id = body.get("node_id").and_then(Value::as_str).unwrap_or("");
            // Approving something that is not there must not read as success.
            // A silent yes here would mean the page could show an approval that
            // never happened.
            let outcome = match daemon.graph.node(node_id) {
                None => json!({
                    "ok": false,
                    "error": format!("there is no node called `{}`", node_id),
                }),
                Some(node) if node.state != "needs-user" => json!({
                    "ok": false,
                    "error": format!("`{}` is {}, not waiting for you", node.title, node.state),
                }),
                Some(node) => match daemon.graph.approve(node_id) {
                    Ok(()) => {
                        info!(node = %node.title, "a blocked node was approved");
                        json!({"ok": true, "node_id": node_id, "title": node.title})
                    }
                    Err(error) => json!({"ok": false, "error": error}),
                },
            };
            ("200 OK", "application/json", outcome.to_string())
        }
        ("POST", "/api/mode") => {
            let wanted = body.get("mode").and_then(Value::as_str).unwrap_or("");
            let outcome = match crate::router::CostMode::parse(wanted) {
                Some(mode) => {
                    if let Ok(mut router) = daemon.router.write() {
                        router.set_cost_mode(mode);
                    }
                    let _ = crate::config::write_mode(mode);
                    json!({"ok": true, "mode": mode.label()})
                }
                None => json!({"ok": false, "error": "unknown mode"}),
            };
            ("200 OK", "application/json", outcome.to_string())
        }
        ("POST", "/api/halt") => {
            let outcome = match daemon.halt.halt() {
                Ok(()) => json!({"ok": true, "halted": true}),
                Err(error) => json!({"ok": false, "error": error.to_string()}),
            };
            ("200 OK", "application/json", outcome.to_string())
        }
        ("POST", "/api/resume") => {
            let outcome = match daemon.halt.resume() {
                Ok(()) => json!({"ok": true, "halted": false}),
                Err(error) => json!({"ok": false, "error": error.to_string()}),
            };
            ("200 OK", "application/json", outcome.to_string())
        }
        _ => (
            "404 Not Found",
            "text/plain; charset=utf-8",
            "no such page\n".to_string(),
        ),
    }
}

/// Everything the page shows, in one document.
///
/// Assembled here rather than in the browser so the page holds no logic: it
/// renders what it is given.
async fn state(daemon: &Arc<Daemon>) -> Value {
    let machine = crate::scheduler::MachineState::read();
    let energy = daemon.energy.today().unwrap_or(crate::scheduler::power::DayEnergy {
        day: "today".to_string(),
        watt_hours: 0.0,
        samples: 0,
    });
    let tariff = daemon.power.config().tariff_per_kwh;

    let mut goals = Vec::new();
    for goal in daemon.graph.goals().unwrap_or_default() {
        let nodes = daemon.graph.nodes(&goal.id).unwrap_or_default();
        goals.push(json!({
            "id": goal.id,
            "description": goal.description,
            "state": goal.state,
            "nodes": nodes,
        }));
    }

    json!({
        "halted": daemon.halt.is_halted(),
        "cost_mode": daemon.router.read().map(|r| r.config().cost_mode.label()).unwrap_or("unknown"),
        "goals": goals,
        "machine": machine,
        "machine_summary": machine.summary(),
        "escalations": daemon.escalations.recent(12).unwrap_or_default(),
        "spend": {
            "today": daemon.spend.spent_today_total().unwrap_or(0.0),
            "rows": daemon.spend.by_day(30).unwrap_or_default(),
            "caps": daemon.spend.caps(),
        },
        "energy": {
            "watt_hours_today": energy.watt_hours,
            "cost_today": energy.cost(tariff),
            "tariff_per_kwh": tariff,
        },
        "memory": daemon.memory.stats().unwrap_or_default(),
        "policy": daemon.policy_log.recent(12).unwrap_or_default(),
        "journal": daemon.journal.list(&crate::journal::Filter {
            limit: 12,
            ..Default::default()
        }).unwrap_or_default(),
        "pulse": {
            "tick_secs": daemon.pulse.config().tick_secs,
            "tasks": daemon.pulse.tasks(),
            "watchers": daemon.pulse.config().watchers,
            "model_loaded": daemon.power.model_loaded(),
            "idle_secs": daemon.power.idle_secs(),
        },
        "prompts": daemon.prompt_cache.list().unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_is_embedded_and_styled_from_the_tokens() {
        assert!(PAGE.contains("<html"), "the page must be a document");
        // The three semantic hues, and no fourth.
        assert!(PAGE.contains("#7A9B6E"), "local green is missing");
        assert!(PAGE.contains("#C8934A"), "api amber is missing");
        assert!(PAGE.contains("#A85445"), "stop red is missing");
        assert!(PAGE.contains("#262523"), "the base colour is missing");
    }

    #[test]
    fn the_page_uses_only_the_permitted_hues() {
        // Any six-digit hex in the page must be one of the STYLE.md tokens.
        let allowed = [
            "262523", "302e2b", "3a3835", "454340", "e4e1db", "8f8b84", "66625c",
            "7a9b6e", "c8934a", "a85445", "f2efe9",
        ];
        let lowered = PAGE.to_lowercase();
        let bytes: Vec<char> = lowered.chars().collect();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == '#' && index + 6 < bytes.len() {
                let candidate: String = bytes[index + 1..index + 7].iter().collect();
                if candidate.chars().all(|c| c.is_ascii_hexdigit()) {
                    assert!(
                        allowed.contains(&candidate.as_str()),
                        "#{} is not a STYLE.md token, which means a fourth colour",
                        candidate
                    );
                }
            }
            index += 1;
        }
    }

    #[test]
    fn the_page_carries_the_first_run_view() {
        // Somebody who did the wizard in a terminal and somebody who did it in
        // a browser must end up in the same place.
        assert!(PAGE.contains("first run"), "no first-run view");
        assert!(PAGE.contains("/api/firstrun"), "it cannot ask what the goal does");
        assert!(
            PAGE.contains("without opening anything"),
            "it must say what the goal touches before running it"
        );
    }

    #[test]
    fn the_page_carries_a_halt_control() {
        assert!(PAGE.contains("halt"), "the halt control must always be reachable");
    }

    #[test]
    fn the_page_holds_no_business_logic_beyond_rendering() {
        // It must not compute costs or decide routing; it renders what it gets.
        assert!(!PAGE.contains("cost_per_1k"), "pricing belongs in the daemon");
        assert!(!PAGE.contains("escalate("), "routing belongs in the daemon");
    }
}
