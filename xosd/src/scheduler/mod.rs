//! XOS Pulse — heartbeat, cron and watchers.
//!
//! Wakes work on a schedule or on a system event, under a power and budget
//! policy that keeps an idle machine idle.
//!
//! Pulse orchestrates and nothing else. It decides that something should happen
//! and asks the daemon to do it, so every action still passes the policy engine.
//! It holds no provider handle and no tool.
//!
//! # Power is not a footnote
//!
//! A GTX 1080 and an old PSU idling around the clock for a heartbeat draw
//! roughly 60 to 100W, which is £150-200 a year at UK rates. "Free local AI"
//! stops being free, and the whole argument for running locally goes with it.
//! So the model is unloaded from VRAM after an idle timeout and reloaded on
//! demand, the GPU is left to drop to its lowest power state between ticks, and
//! the energy actually used is measured and recorded so it can be shown next to
//! API spend. That comparison is the honest version of the local-versus-cloud
//! trade, and it needs a real number on both sides.

pub mod machine;
pub mod power;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub use machine::MachineState;
pub use power::{EnergyLog, PowerManager};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PulseConfig {
    /// How often the heartbeat runs.
    #[serde(default = "default_tick_secs")]
    pub tick_secs: u64,
    /// How many graph nodes one tick may start.
    #[serde(default = "default_nodes_per_tick")]
    pub nodes_per_tick: usize,
    #[serde(default)]
    pub tasks: Vec<ScheduledTask>,
    #[serde(default)]
    pub watchers: Vec<Watcher>,
    /// Send desktop notifications for things needing attention.
    #[serde(default = "default_true")]
    pub notifications: bool,
    #[serde(default)]
    pub power: power::PowerConfig,
}

fn default_tick_secs() -> u64 {
    300
}
fn default_nodes_per_tick() -> usize {
    1
}
fn default_true() -> bool {
    true
}

impl Default for PulseConfig {
    fn default() -> Self {
        Self {
            tick_secs: default_tick_secs(),
            nodes_per_tick: default_nodes_per_tick(),
            tasks: Vec::new(),
            watchers: Vec::new(),
            notifications: true,
            power: power::PowerConfig::default(),
        }
    }
}

/// Something to do on a schedule.
///
/// Two shapes, because those are the two things people actually want: every so
/// many minutes, or at a time of day.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub name: String,
    /// What to ask XOS to do.
    pub goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_minutes: Option<u64>,
    /// Local time, "HH:MM".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

impl ScheduledTask {
    /// Is this task due, given when it last ran?
    ///
    /// `now_minutes` is minutes since the epoch, and `minute_of_day` the local
    /// clock, both passed in so this is testable without waiting for a clock.
    pub fn due(&self, now_minutes: i64, minute_of_day: u32, last_run: Option<i64>) -> bool {
        if let Some(every) = self.every_minutes {
            let every = every.max(1) as i64;
            return match last_run {
                None => true,
                Some(last) => now_minutes - last >= every,
            };
        }
        if let Some(at) = &self.at {
            let Some(wanted) = parse_clock(at) else {
                return false;
            };
            // Due within the minute it names, and only once that day.
            if minute_of_day != wanted {
                return false;
            }
            return match last_run {
                None => true,
                // Not again within the same hour, which covers a tick interval
                // firing twice inside the named minute.
                Some(last) => now_minutes - last >= 60,
            };
        }
        false
    }
}

fn parse_clock(text: &str) -> Option<u32> {
    let (hours, minutes) = text.split_once(':')?;
    let hours: u32 = hours.trim().parse().ok()?;
    let minutes: u32 = minutes.trim().parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(hours * 60 + minutes)
}

/// Run something when a path changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Watcher {
    pub name: String,
    pub path: PathBuf,
    pub goal: String,
}

/// What one tick decided to do. Pulse produces this; the daemon carries it out,
/// so nothing here touches a tool or a provider.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum TickAction {
    /// Advance the task graph.
    AdvanceGraph { nodes: usize },
    /// Start a scheduled task as a goal.
    StartTask { name: String, goal: String },
    /// A watched path changed.
    WatchFired { name: String, goal: String },
    /// Release the GPU, because nothing has needed it for a while.
    UnloadModel { idle_secs: u64 },
}

#[derive(Debug, Clone, Serialize)]
pub struct TickReport {
    pub ran: bool,
    pub reason: String,
    pub actions: Vec<TickAction>,
}

pub struct Pulse {
    config: PulseConfig,
    /// Minute-of-epoch each task last ran.
    last_run: std::sync::Mutex<std::collections::HashMap<String, i64>>,
    /// Modification time last seen for each watched path.
    seen: std::sync::Mutex<std::collections::HashMap<String, i64>>,
}

impl Pulse {
    pub fn new(config: PulseConfig) -> Self {
        Self {
            config,
            last_run: std::sync::Mutex::new(std::collections::HashMap::new()),
            seen: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn config(&self) -> &PulseConfig {
        &self.config
    }

    /// Decide what this tick should do.
    ///
    /// `halted` comes from the kill switch, and a halted system advances
    /// nothing: no graph work, no scheduled tasks, no watchers. The tick still
    /// happens, so status stays truthful, but it does nothing.
    pub fn tick(&self, halted: bool, idle_secs: u64) -> TickReport {
        if halted {
            return TickReport {
                ran: false,
                reason: "halted, so nothing was advanced".to_string(),
                actions: Vec::new(),
            };
        }

        let mut actions = Vec::new();
        let now_minutes = unix_now() / 60;
        let minute_of_day = local_minute_of_day();

        for task in &self.config.tasks {
            let last = self
                .last_run
                .lock()
                .ok()
                .and_then(|map| map.get(&task.name).copied());
            if task.due(now_minutes, minute_of_day, last) {
                if let Ok(mut map) = self.last_run.lock() {
                    map.insert(task.name.clone(), now_minutes);
                }
                actions.push(TickAction::StartTask {
                    name: task.name.clone(),
                    goal: task.goal.clone(),
                });
            }
        }

        for watcher in &self.config.watchers {
            if let Some(changed) = self.changed(watcher) {
                actions.push(TickAction::WatchFired {
                    name: watcher.name.clone(),
                    goal: format!("{} ({} changed)", watcher.goal, changed),
                });
            }
        }

        actions.push(TickAction::AdvanceGraph {
            nodes: self.config.nodes_per_tick.max(1),
        });

        if self.config.power.unload_after_idle_secs > 0
            && idle_secs >= self.config.power.unload_after_idle_secs
        {
            actions.push(TickAction::UnloadModel { idle_secs });
        }

        TickReport {
            ran: true,
            reason: format!("{} actions", actions.len()),
            actions,
        }
    }

    /// Has a watched path changed since the last look?
    fn changed(&self, watcher: &Watcher) -> Option<String> {
        let modified = std::fs::metadata(&watcher.path)
            .ok()?
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_secs() as i64;

        let key = watcher.path.display().to_string();
        let mut seen = self.seen.lock().ok()?;
        match seen.get(&key).copied() {
            // First sight is not a change; it is the baseline.
            None => {
                seen.insert(key, modified);
                None
            }
            Some(previous) if modified > previous => {
                seen.insert(key.clone(), modified);
                Some(key)
            }
            _ => None,
        }
    }

    pub fn tasks(&self) -> &[ScheduledTask] {
        &self.config.tasks
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64
}

/// Minutes since local midnight.
///
/// Read from the system rather than computed, so a timezone or a daylight
/// saving change is the operating system's problem rather than ours.
fn local_minute_of_day() -> u32 {
    let output = std::process::Command::new("date").args(["+%H:%M"]).output();
    match output {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            parse_clock(text.trim()).unwrap_or(0)
        }
        _ => {
            // Fall back to UTC from the epoch.
            ((unix_now() % 86_400) / 60) as u32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every(name: &str, minutes: u64) -> ScheduledTask {
        ScheduledTask {
            name: name.to_string(),
            goal: "do the thing".to_string(),
            every_minutes: Some(minutes),
            at: None,
        }
    }

    fn daily(name: &str, at: &str) -> ScheduledTask {
        ScheduledTask {
            name: name.to_string(),
            goal: "the morning brief".to_string(),
            every_minutes: None,
            at: Some(at.to_string()),
        }
    }

    #[test]
    fn an_interval_task_is_due_the_first_time() {
        assert!(every("backup", 30).due(1000, 0, None));
    }

    #[test]
    fn an_interval_task_waits_for_its_interval() {
        let task = every("backup", 30);
        assert!(!task.due(1010, 0, Some(1000)), "only 10 minutes have passed");
        assert!(task.due(1030, 0, Some(1000)), "30 minutes have passed");
    }

    #[test]
    fn a_daily_task_fires_in_the_minute_it_names() {
        let task = daily("brief", "07:30");
        let half_past_seven = 7 * 60 + 30;
        assert!(task.due(1000, half_past_seven, None));
        assert!(!task.due(1000, half_past_seven + 1, None), "a minute later is not it");
    }

    #[test]
    fn a_daily_task_does_not_fire_twice_in_the_same_minute() {
        let task = daily("brief", "07:30");
        let half_past_seven = 7 * 60 + 30;
        assert!(
            !task.due(1000, half_past_seven, Some(999)),
            "a tick landing twice inside the minute must not run it twice"
        );
    }

    #[test]
    fn a_malformed_time_never_fires() {
        let task = daily("brief", "nonsense");
        assert!(!task.due(1000, 0, None));
        assert!(!daily("brief", "25:00").due(1000, 25 * 60, None));
    }

    #[test]
    fn a_task_with_no_schedule_never_fires() {
        let task = ScheduledTask {
            name: "orphan".to_string(),
            goal: "nothing".to_string(),
            every_minutes: None,
            at: None,
        };
        assert!(!task.due(1000, 500, None));
    }

    #[test]
    fn a_halted_system_advances_nothing() {
        let pulse = Pulse::new(PulseConfig {
            tasks: vec![every("backup", 1)],
            ..Default::default()
        });
        let report = pulse.tick(true, 0);
        assert!(!report.ran);
        assert!(report.actions.is_empty(), "a halt must stop every action");
        assert!(report.reason.contains("halted"));
    }

    #[test]
    fn a_tick_advances_the_graph() {
        let pulse = Pulse::new(PulseConfig::default());
        let report = pulse.tick(false, 0);
        assert!(report.ran);
        assert!(report
            .actions
            .iter()
            .any(|action| matches!(action, TickAction::AdvanceGraph { .. })));
    }

    #[test]
    fn a_due_task_is_started_once_not_every_tick() {
        let pulse = Pulse::new(PulseConfig {
            tasks: vec![every("backup", 60)],
            ..Default::default()
        });
        let first = pulse.tick(false, 0);
        assert!(first
            .actions
            .iter()
            .any(|a| matches!(a, TickAction::StartTask { .. })));

        let second = pulse.tick(false, 0);
        assert!(
            !second
                .actions
                .iter()
                .any(|a| matches!(a, TickAction::StartTask { .. })),
            "a task on a 60 minute interval must not run on the next tick"
        );
    }

    #[test]
    fn an_idle_machine_is_asked_to_release_the_gpu() {
        let pulse = Pulse::new(PulseConfig {
            power: power::PowerConfig {
                unload_after_idle_secs: 600,
                ..Default::default()
            },
            ..Default::default()
        });
        let busy = pulse.tick(false, 60);
        assert!(!busy
            .actions
            .iter()
            .any(|a| matches!(a, TickAction::UnloadModel { .. })));

        let idle = pulse.tick(false, 900);
        assert!(
            idle.actions
                .iter()
                .any(|a| matches!(a, TickAction::UnloadModel { .. })),
            "an idle GPU costs real money and must be released"
        );
    }

    #[test]
    fn unloading_can_be_switched_off() {
        let pulse = Pulse::new(PulseConfig {
            power: power::PowerConfig {
                unload_after_idle_secs: 0,
                ..Default::default()
            },
            ..Default::default()
        });
        let report = pulse.tick(false, 100_000);
        assert!(!report
            .actions
            .iter()
            .any(|a| matches!(a, TickAction::UnloadModel { .. })));
    }

    #[test]
    fn a_watcher_fires_on_change_but_not_on_first_sight() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("watched.txt");
        std::fs::write(&path, "first").expect("write");

        let pulse = Pulse::new(PulseConfig {
            watchers: vec![Watcher {
                name: "inbox".to_string(),
                path: path.clone(),
                goal: "sort the inbox".to_string(),
            }],
            ..Default::default()
        });

        let first = pulse.tick(false, 0);
        assert!(
            !first
                .actions
                .iter()
                .any(|a| matches!(a, TickAction::WatchFired { .. })),
            "the first look establishes a baseline rather than firing"
        );

        // Move the file's timestamp on, as a change would.
        let later = SystemTime::now() + std::time::Duration::from_secs(120);
        let file = std::fs::OpenOptions::new().write(true).open(&path).expect("open");
        file.set_times(std::fs::FileTimes::new().set_modified(later))
            .expect("set times");

        let second = pulse.tick(false, 0);
        assert!(
            second
                .actions
                .iter()
                .any(|a| matches!(a, TickAction::WatchFired { .. })),
            "a changed file must fire the watcher"
        );
    }

    #[test]
    fn a_missing_watched_path_is_not_an_error() {
        let pulse = Pulse::new(PulseConfig {
            watchers: vec![Watcher {
                name: "gone".to_string(),
                path: PathBuf::from("/nonexistent/path/xos"),
                goal: "nothing".to_string(),
            }],
            ..Default::default()
        });
        let report = pulse.tick(false, 0);
        assert!(report.ran);
    }
}
