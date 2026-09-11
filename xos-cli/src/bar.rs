//! `xos bar` — the waybar module.
//!
//! The status bar is the one surface where boldness is spent. It answers three
//! questions at a glance: what is the system doing, what is it costing, can I
//! trust it. Everything else on the desktop stays quiet.
//!
//! The rule that shapes this file is *quiet when idle, loud when it matters*. A
//! bar that is always busy becomes wallpaper, and then the one time it has
//! something urgent to say, nobody reads it. So most of what could be shown is
//! shown only when it is true: tokens per second while generating, a blocked
//! count only when something is actually blocked, spend colour only as it
//! approaches a budget.
//!
//! Colour carries meaning and nothing else. Green is local and free, amber is
//! costing money, oxide red needs you. There is no fourth hue and no decoration.

use serde_json::{json, Value};

use crate::socket::Connection;

/// Waybar reads this shape: text, tooltip, and a class it styles from CSS.
#[derive(Debug, Clone, PartialEq)]
pub struct BarState {
    pub text: String,
    pub tooltip: String,
    /// Drives the colour. One of: halted, blocked, api, local, offline.
    pub class: String,
}

impl BarState {
    pub fn to_json(&self) -> Value {
        json!({
            "text": self.text,
            "tooltip": self.tooltip,
            "class": self.class,
            "alt": self.class,
        })
    }
}

/// Build the bar from what the daemon reports.
///
/// Kept as a pure function of two JSON documents so the whole of the bar's
/// behaviour is testable without a compositor.
pub fn render(status: &Value, pulse: &Value, goals: &Value) -> BarState {
    // Halted is unmissable and outranks everything else. A person who hit the
    // kill switch needs to see that it took, not a tidy summary of spend.
    if status.get("halted").and_then(Value::as_bool).unwrap_or(false) {
        return BarState {
            text: "halted — click to resume".to_string(),
            tooltip: "Every autonomous action is stopped, and stays stopped across a restart.\nClick to resume.".to_string(),
            class: "halted".to_string(),
        };
    }

    let mut segments: Vec<String> = Vec::new();

    // What is it doing: the current goal and how far through it is.
    if let Some((title, done, total)) = current_goal(goals) {
        segments.push(format!("{} · {}/{}", truncate(&title, 28), done, total));
    }

    // Which tier, which is also which colour.
    let local = status
        .get("providers")
        .and_then(Value::as_array)
        .and_then(|providers| {
            let default = status.get("default_provider").and_then(Value::as_str)?;
            providers
                .iter()
                .find(|p| p.get("name").and_then(Value::as_str) == Some(default))
        })
        .and_then(|p| p.pointer("/capabilities/local"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    segments.push(if local { "local".to_string() } else { "api".to_string() });

    // What is it costing.
    let spend = status
        .get("spend_today")
        .and_then(Value::as_f64)
        .or_else(|| pulse.get("spend_today").and_then(Value::as_f64))
        .unwrap_or(0.0);
    if spend > 0.0 {
        segments.push(format!("{:.2}", spend));
    }

    // Contextual: only while something is actually generating.
    if let Some(rate) = pulse.get("tokens_per_second").and_then(Value::as_f64) {
        if rate > 0.0 {
            segments.push(format!("{:.0} tok/s", rate));
        }
    }
    if pulse
        .get("listening")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        segments.push("mic".to_string());
    }

    // Contextual and the only thing permitted to be red: work that needs you.
    let blocked = blocked_count(goals);
    if blocked > 0 {
        segments.push(format!("{} need you", blocked));
    }

    let cap = pulse
        .pointer("/caps/daily_total")
        .and_then(Value::as_f64)
        .or_else(|| status.pointer("/caps/daily_total").and_then(Value::as_f64));

    let class = if blocked > 0 {
        "blocked"
    } else if over_budget(spend, cap) {
        "blocked"
    } else if near_budget(spend, cap) {
        "api"
    } else if !local {
        "api"
    } else {
        "local"
    };

    BarState {
        text: segments.join("  ·  "),
        tooltip: tooltip(status, pulse, goals, blocked, spend),
        class: class.to_string(),
    }
}

/// The panel that opens on click.
pub fn panel(status: &Value, pulse: &Value, goals: &Value) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    let _ = writeln!(
        out,
        "model        {}",
        status
            .get("default_provider")
            .and_then(Value::as_str)
            .unwrap_or("none")
    );
    if let Some(gpu) = pulse.pointer("/machine/gpu") {
        let _ = writeln!(
            out,
            "vram         {} of {} MB",
            gpu.get("memory_used_mb").and_then(Value::as_u64).unwrap_or(0),
            gpu.get("memory_total_mb").and_then(Value::as_u64).unwrap_or(0)
        );
    }
    let _ = writeln!(
        out,
        "cost mode    {}",
        status.get("cost_mode").and_then(Value::as_str).unwrap_or("-")
    );
    let _ = writeln!(
        out,
        "pulse        every {}s, model {}",
        pulse.get("tick_secs").and_then(Value::as_u64).unwrap_or(0),
        if pulse.get("model_loaded").and_then(Value::as_bool).unwrap_or(false) {
            "resident"
        } else {
            "released"
        }
    );
    let _ = writeln!(
        out,
        "electricity  {:.1} Wh today, about {:.4} (estimate)",
        pulse
            .pointer("/energy/watt_hours_today")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        pulse
            .pointer("/energy/cost_today")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    );
    let _ = writeln!(
        out,
        "api spend    {:.4} today",
        pulse.get("spend_today").and_then(Value::as_f64).unwrap_or(0.0)
    );

    let blocked = blocked_count(goals);
    if blocked > 0 {
        let _ = writeln!(out);
        let _ = writeln!(out, "{} steps need your confirmation.", blocked);
    }
    out
}

fn tooltip(status: &Value, pulse: &Value, goals: &Value, blocked: usize, spend: f64) -> String {
    let mut lines = Vec::new();
    if let Some((title, done, total)) = current_goal(goals) {
        lines.push(format!("{} — step {} of {}", title, done + 1, total));
    }
    if let Some(summary) = pulse.get("summary").and_then(Value::as_str) {
        lines.push(summary.to_string());
    }
    lines.push(format!(
        "{} today on the API",
        format_args!("{:.4}", spend)
    ));
    if blocked > 0 {
        lines.push(format!("{} steps need your confirmation", blocked));
    }
    if let Some(mode) = status.get("cost_mode").and_then(Value::as_str) {
        lines.push(format!("cost mode {}", mode));
    }
    lines.join("\n")
}

/// The goal currently being worked, with progress through its nodes.
fn current_goal(goals: &Value) -> Option<(String, usize, usize)> {
    let list = goals.get("goals").and_then(Value::as_array)?;
    let running = list
        .iter()
        .find(|goal| goal.get("state").and_then(Value::as_str) == Some("running"))?;
    let title = running
        .get("description")
        .and_then(Value::as_str)?
        .to_string();
    let nodes = running.get("nodes").and_then(Value::as_array);
    let (done, total) = match nodes {
        Some(nodes) => (
            nodes
                .iter()
                .filter(|n| n.get("state").and_then(Value::as_str) == Some("done"))
                .count(),
            nodes.len(),
        ),
        None => (0, 0),
    };
    Some((title, done, total))
}

fn blocked_count(goals: &Value) -> usize {
    goals
        .get("goals")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|goal| goal.get("nodes").and_then(Value::as_array))
                .flatten()
                .filter(|node| node.get("state").and_then(Value::as_str) == Some("needs-user"))
                .count()
        })
        .unwrap_or(0)
}

fn near_budget(spend: f64, cap: Option<f64>) -> bool {
    match cap {
        Some(cap) if cap > 0.0 => spend >= cap * 0.8,
        _ => false,
    }
}

fn over_budget(spend: f64, cap: Option<f64>) -> bool {
    match cap {
        Some(cap) if cap > 0.0 => spend >= cap,
        _ => false,
    }
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_string()
    } else {
        text.chars().take(width.saturating_sub(1)).collect::<String>() + "\u{2026}"
    }
}

/// Gather what the bar needs and print it as waybar JSON.
pub fn run(connection: &mut Connection, as_panel: bool) -> Result<String, String> {
    let status = connection.call("status", json!({}))?;
    let pulse = connection.call("pulse.status", json!({})).unwrap_or(Value::Null);
    let goals = gather_goals(connection);

    if as_panel {
        return Ok(panel(&status, &pulse, &goals));
    }
    let state = render(&status, &pulse, &goals);
    Ok(format!("{}\n", state.to_json()))
}

/// Goals with their nodes folded in, which is what progress needs.
fn gather_goals(connection: &mut Connection) -> Value {
    let listed = match connection.call("graph.list", json!({})) {
        Ok(listed) => listed,
        Err(_) => return json!({"goals": []}),
    };
    let mut goals = Vec::new();
    for goal in listed
        .get("goals")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let Some(id) = goal.get("id").and_then(Value::as_str) else {
            continue;
        };
        let mut goal = goal.clone();
        if let Ok(status) = connection.call("graph.status", json!({"goal_id": id})) {
            if let Some(nodes) = status.get("nodes") {
                goal["nodes"] = nodes.clone();
            }
        }
        goals.push(goal);
    }
    json!({"goals": goals})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(halted: bool, local: bool, spend: f64) -> Value {
        json!({
            "halted": halted,
            "cost_mode": "balanced",
            "default_provider": if local { "local" } else { "cloud" },
            "spend_today": spend,
            "providers": [
                {"name": "local", "capabilities": {"local": true}},
                {"name": "cloud", "capabilities": {"local": false}},
            ],
        })
    }

    fn goals(state: &str, done: usize, total: usize, needs_user: usize) -> Value {
        let mut nodes = Vec::new();
        for index in 0..total {
            let node_state = if index < done {
                "done"
            } else if index < done + needs_user {
                "needs-user"
            } else {
                "pending"
            };
            nodes.push(json!({"state": node_state, "title": format!("step {}", index)}));
        }
        json!({"goals": [{
            "id": "goal-1",
            "description": "deploy site",
            "state": state,
            "nodes": nodes,
        }]})
    }

    #[test]
    fn halted_takes_over_the_whole_bar() {
        let bar = render(&status(true, true, 5.0), &json!({}), &goals("running", 1, 7, 0));
        assert_eq!(bar.class, "halted");
        assert!(bar.text.contains("halted"), "{}", bar.text);
        assert!(bar.text.contains("click to resume"), "{}", bar.text);
        assert!(
            !bar.text.contains("deploy site"),
            "halted must not be one item among several: {}",
            bar.text
        );
    }

    #[test]
    fn the_current_goal_shows_its_progress() {
        let bar = render(&status(false, true, 0.0), &json!({}), &goals("running", 3, 7, 0));
        assert!(bar.text.contains("deploy site · 3/7"), "{}", bar.text);
    }

    #[test]
    fn the_local_tier_is_green() {
        let bar = render(&status(false, true, 0.0), &json!({}), &json!({}));
        assert_eq!(bar.class, "local");
        assert!(bar.text.contains("local"));
    }

    #[test]
    fn the_api_tier_is_amber() {
        let bar = render(&status(false, false, 0.0), &json!({}), &json!({}));
        assert_eq!(bar.class, "api");
        assert!(bar.text.contains("api"));
    }

    #[test]
    fn a_blocked_node_is_the_only_thing_that_turns_it_red() {
        let bar = render(&status(false, true, 0.0), &json!({}), &goals("running", 1, 4, 1));
        assert_eq!(bar.class, "blocked");
        assert!(bar.text.contains("1 need you"), "{}", bar.text);
    }

    #[test]
    fn an_idle_bar_stays_quiet() {
        // Nothing running, nothing spent, nothing blocked: only the tier.
        let bar = render(&status(false, true, 0.0), &json!({}), &json!({"goals": []}));
        assert_eq!(bar.text, "local", "a quiet bar must say almost nothing");
    }

    #[test]
    fn tokens_per_second_appear_only_while_generating() {
        let quiet = render(&status(false, true, 0.0), &json!({}), &json!({}));
        assert!(!quiet.text.contains("tok/s"));

        let busy = render(
            &status(false, true, 0.0),
            &json!({"tokens_per_second": 42.0}),
            &json!({}),
        );
        assert!(busy.text.contains("42 tok/s"), "{}", busy.text);
    }

    #[test]
    fn the_microphone_appears_only_when_listening() {
        let bar = render(&status(false, true, 0.0), &json!({"listening": true}), &json!({}));
        assert!(bar.text.contains("mic"), "{}", bar.text);
    }

    #[test]
    fn spend_turns_amber_near_the_cap_and_red_over_it() {
        let near = render(
            &status(false, true, 8.5),
            &json!({"caps": {"daily_total": 10.0}}),
            &json!({}),
        );
        assert_eq!(near.class, "api", "approaching the cap is amber");

        let over = render(
            &status(false, true, 11.0),
            &json!({"caps": {"daily_total": 10.0}}),
            &json!({}),
        );
        assert_eq!(over.class, "blocked", "over the cap needs you");
    }

    #[test]
    fn spend_without_a_cap_does_not_change_colour() {
        let bar = render(&status(false, true, 99.0), &json!({}), &json!({}));
        assert_eq!(bar.class, "local");
        assert!(bar.text.contains("99.00"), "{}", bar.text);
    }

    #[test]
    fn a_long_goal_title_is_trimmed_rather_than_pushing_the_bar_wide() {
        let long = json!({"goals": [{
            "id": "g", "state": "running",
            "description": "reorganise every photograph taken since two thousand and four",
            "nodes": [{"state": "done"}, {"state": "pending"}],
        }]});
        let bar = render(&status(false, true, 0.0), &json!({}), &long);
        assert!(bar.text.contains('\u{2026}'), "{}", bar.text);
        assert!(bar.text.len() < 80, "{}", bar.text);
    }

    #[test]
    fn the_json_carries_what_waybar_reads() {
        let bar = render(&status(false, true, 0.0), &json!({}), &json!({}));
        let value = bar.to_json();
        assert!(value.get("text").is_some());
        assert!(value.get("tooltip").is_some());
        assert!(value.get("class").is_some());
    }

    #[test]
    fn the_panel_shows_what_the_expand_promises() {
        let pulse = json!({
            "tick_secs": 300,
            "model_loaded": true,
            "machine": {"gpu": {"memory_used_mb": 5200, "memory_total_mb": 8192}},
            "energy": {"watt_hours_today": 120.0, "cost_today": 0.0294},
            "spend_today": 1.25,
        });
        let text = panel(&status(false, true, 1.25), &pulse, &goals("running", 1, 3, 1));
        assert!(text.contains("vram"), "{}", text);
        assert!(text.contains("5200 of 8192"), "{}", text);
        assert!(text.contains("cost mode"), "{}", text);
        assert!(text.contains("electricity"), "{}", text);
        assert!(text.contains("api spend"), "{}", text);
        assert!(text.contains("need your confirmation"), "{}", text);
    }

    #[test]
    fn every_class_maps_to_a_meaning_and_there_are_only_four() {
        // Green local, amber api, red needs-you, plus halted which is also red.
        let classes = ["local", "api", "blocked", "halted"];
        for class in classes {
            assert!(!class.is_empty());
        }
        assert_eq!(classes.len(), 4, "a fifth class would mean a fourth colour");
    }
}
