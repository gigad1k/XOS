//! Power management, and the energy number that makes the local case honest.
//!
//! Running a model locally is only free if the machine is not left drawing
//! power to do nothing. A GTX 1080 idling around the clock costs real money
//! every year, so XOS releases the GPU when nothing needs it, lets it drop to
//! its lowest power state between ticks, and measures what it actually used.
//!
//! The measurement is the point. Mission Control shows watt-hours beside API
//! spend, and that comparison is only worth anything if both numbers are real.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS energy (
    day        TEXT PRIMARY KEY,
    watt_hours REAL NOT NULL DEFAULT 0,
    samples    INTEGER NOT NULL DEFAULT 0
);
";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerConfig {
    /// Release the GPU after this long with nothing to do. Zero disables it.
    #[serde(default = "default_idle")]
    pub unload_after_idle_secs: u64,
    /// What the rest of the machine draws at idle, in watts. Used with the
    /// GPU's own reading to estimate the whole box.
    #[serde(default = "default_baseline")]
    pub baseline_watts: f64,
    /// Cost per kilowatt-hour, for turning watt-hours into money.
    #[serde(default = "default_tariff")]
    pub tariff_per_kwh: f64,
    /// Suspend between scheduled work and wake with the real-time clock, so
    /// overnight work does not need the machine left on. Off by default: it
    /// interrupts whatever the person is doing.
    #[serde(default)]
    pub suspend_between_work: bool,
}

fn default_idle() -> u64 {
    900
}
fn default_baseline() -> f64 {
    45.0
}
fn default_tariff() -> f64 {
    0.245
}

impl Default for PowerConfig {
    fn default() -> Self {
        Self {
            unload_after_idle_secs: default_idle(),
            baseline_watts: default_baseline(),
            tariff_per_kwh: default_tariff(),
            suspend_between_work: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DayEnergy {
    pub day: String,
    pub watt_hours: f64,
    pub samples: i64,
}

impl DayEnergy {
    pub fn cost(&self, tariff_per_kwh: f64) -> f64 {
        (self.watt_hours / 1000.0) * tariff_per_kwh
    }
}

/// Watt-hours used, by day.
pub struct EnergyLog {
    connection: Mutex<Connection>,
}

impl EnergyLog {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {}", parent.display(), e))?;
        }
        let connection = Connection::open(path)
            .map_err(|e| format!("cannot open {}: {}", path.display(), e))?;
        connection
            .execute_batch(SCHEMA)
            .map_err(|e| format!("cannot prepare the energy log: {}", e))?;
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

    /// Add the energy used since the last sample.
    ///
    /// Watts multiplied by hours. Sampling at the tick interval means each
    /// sample stands for the interval it covers, which is an approximation, and
    /// an honest one as long as the interval is short relative to how fast load
    /// changes.
    pub fn add(&self, watts: f64, seconds: f64) -> Result<f64, String> {
        let watt_hours = watts * (seconds / 3600.0);
        let connection = self
            .connection
            .lock()
            .map_err(|_| "energy log lock".to_string())?;
        connection
            .execute(
                "INSERT INTO energy (day, watt_hours, samples)
                 VALUES (date('now'), ?1, 1)
                 ON CONFLICT(day) DO UPDATE SET
                   watt_hours = watt_hours + excluded.watt_hours,
                   samples = samples + 1",
                params![watt_hours],
            )
            .map_err(|e| format!("cannot record energy: {}", e))?;
        Ok(watt_hours)
    }

    pub fn today(&self) -> Result<DayEnergy, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "energy log lock".to_string())?;
        connection
            .query_row(
                "SELECT day, watt_hours, samples FROM energy WHERE day = date('now')",
                [],
                |row| {
                    Ok(DayEnergy {
                        day: row.get(0)?,
                        watt_hours: row.get(1)?,
                        samples: row.get(2)?,
                    })
                },
            )
            .or_else(|_| {
                Ok(DayEnergy {
                    day: "today".to_string(),
                    watt_hours: 0.0,
                    samples: 0,
                })
            })
    }

    pub fn recent(&self, days: u32) -> Result<Vec<DayEnergy>, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "energy log lock".to_string())?;
        let mut statement = connection
            .prepare("SELECT day, watt_hours, samples FROM energy ORDER BY day DESC LIMIT ?1")
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map(params![days.max(1)], |row| {
                Ok(DayEnergy {
                    day: row.get(0)?,
                    watt_hours: row.get(1)?,
                    samples: row.get(2)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }
}

pub struct PowerManager {
    config: PowerConfig,
    /// When something last needed the model.
    last_used: Mutex<i64>,
    /// Whether the model is believed to be resident in VRAM.
    loaded: Mutex<bool>,
}

impl PowerManager {
    pub fn new(config: PowerConfig) -> Self {
        Self {
            config,
            last_used: Mutex::new(unix_now()),
            loaded: Mutex::new(false),
        }
    }

    pub fn config(&self) -> &PowerConfig {
        &self.config
    }

    /// Something used the model, so the idle clock restarts.
    pub fn touch(&self) {
        if let Ok(mut last) = self.last_used.lock() {
            *last = unix_now();
        }
        if let Ok(mut loaded) = self.loaded.lock() {
            *loaded = true;
        }
    }

    pub fn idle_secs(&self) -> u64 {
        self.last_used
            .lock()
            .map(|last| (unix_now() - *last).max(0) as u64)
            .unwrap_or(0)
    }

    pub fn model_loaded(&self) -> bool {
        self.loaded.lock().map(|loaded| *loaded).unwrap_or(false)
    }

    /// Ask the endpoint to drop the model from VRAM.
    ///
    /// Ollama unloads when told to keep the model alive for zero seconds. An
    /// endpoint that does not understand the field ignores it, which costs
    /// nothing, so this is safe to attempt against anything.
    pub fn unload(&self, base_url: &str, model: &str) -> Result<(), String> {
        let root = base_url
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .to_string();
        let body = serde_json::json!({"model": model, "keep_alive": 0});

        let response = ureq_post(&format!("{}/api/generate", root), &body);
        if let Ok(mut loaded) = self.loaded.lock() {
            *loaded = false;
        }
        response
    }

    /// What the machine is drawing right now, in watts.
    ///
    /// The GPU reports itself; the rest of the box is the configured baseline,
    /// because there is no portable way to read a PSU. The estimate is labelled
    /// as an estimate wherever it is shown.
    pub fn watts_now(&self, gpu_watts: Option<f64>) -> f64 {
        self.config.baseline_watts + gpu_watts.unwrap_or(0.0)
    }

    /// The command that would suspend the machine and wake it later.
    ///
    /// Returned rather than run: suspending is not something to do as a side
    /// effect of a status call, and the caller decides.
    pub fn suspend_command(&self, wake_in_secs: u64) -> Option<Vec<String>> {
        if !self.config.suspend_between_work {
            return None;
        }
        Some(vec![
            "rtcwake".to_string(),
            "-m".to_string(),
            "mem".to_string(),
            "-s".to_string(),
            wake_in_secs.to_string(),
        ])
    }
}

fn ureq_post(url: &str, body: &serde_json::Value) -> Result<(), String> {
    // A blocking call on purpose: unloading happens on a tick, not on the hot
    // path, and the daemon should know it finished before saying so.
    let client = std::process::Command::new("curl")
        .args([
            "-s",
            "-m",
            "10",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "-d",
            &body.to_string(),
            url,
        ])
        .output()
        .map_err(|e| format!("cannot reach {}: {}", url, e))?;
    if client.status.success() {
        Ok(())
    } else {
        Err(format!("{} refused the unload", url))
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_manager_is_not_idle() {
        let power = PowerManager::new(PowerConfig::default());
        assert!(power.idle_secs() < 5);
    }

    #[test]
    fn touching_marks_the_model_loaded() {
        let power = PowerManager::new(PowerConfig::default());
        assert!(!power.model_loaded());
        power.touch();
        assert!(power.model_loaded());
    }

    #[test]
    fn watts_add_the_gpu_to_the_baseline() {
        let power = PowerManager::new(PowerConfig {
            baseline_watts: 45.0,
            ..Default::default()
        });
        assert!((power.watts_now(Some(120.0)) - 165.0).abs() < 1e-9);
        assert!(
            (power.watts_now(None) - 45.0).abs() < 1e-9,
            "an unreadable GPU must not become a guess"
        );
    }

    #[test]
    fn suspending_is_off_unless_asked_for() {
        let power = PowerManager::new(PowerConfig::default());
        assert!(power.suspend_command(3600).is_none());
    }

    #[test]
    fn suspending_names_the_wake_time() {
        let power = PowerManager::new(PowerConfig {
            suspend_between_work: true,
            ..Default::default()
        });
        let command = power.suspend_command(3600).expect("a command");
        assert_eq!(command[0], "rtcwake");
        assert!(command.contains(&"3600".to_string()));
    }

    #[test]
    fn energy_accumulates_across_samples() {
        let log = EnergyLog::in_memory().expect("log");
        // 100W for half an hour is 50Wh.
        log.add(100.0, 1800.0).expect("add");
        let today = log.today().expect("today");
        assert!((today.watt_hours - 50.0).abs() < 1e-6, "{}", today.watt_hours);

        log.add(100.0, 1800.0).expect("add");
        let today = log.today().expect("today");
        assert!((today.watt_hours - 100.0).abs() < 1e-6);
        assert_eq!(today.samples, 2);
    }

    #[test]
    fn energy_becomes_money_at_the_tariff() {
        let day = DayEnergy {
            day: "today".to_string(),
            watt_hours: 2000.0,
            samples: 10,
        };
        // 2kWh at 24.5p is 49p.
        assert!((day.cost(0.245) - 0.49).abs() < 1e-9);
    }

    #[test]
    fn an_empty_day_reports_zero_rather_than_failing() {
        let log = EnergyLog::in_memory().expect("log");
        let today = log.today().expect("today");
        assert_eq!(today.watt_hours, 0.0);
        assert_eq!(today.samples, 0);
    }

    #[test]
    fn a_years_idling_is_the_number_the_docs_warn_about() {
        // The argument in the module docs, checked: 80W around the clock.
        let log = EnergyLog::in_memory().expect("log");
        log.add(80.0, 86_400.0).expect("add");
        let day = log.today().expect("today");
        let yearly = day.cost(0.245) * 365.0;
        assert!(
            yearly > 150.0 && yearly < 200.0,
            "a year of idling came to {:.0}, which is not the range the docs claim",
            yearly
        );
    }
}
