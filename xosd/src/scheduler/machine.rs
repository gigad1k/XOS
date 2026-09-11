//! What the machine looks like right now.
//!
//! Collected so the model can reason about the box it lives on: deferring a
//! large download on a metered connection, holding a reindex until the GPU is
//! free, warning before a disk fills. Every reading is optional, because a
//! decade-old laptop and a modern desktop expose different things and XOS has to
//! run on both. A missing reading is reported as missing, never as zero.

use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
pub struct MachineState {
    pub gpu: Option<Gpu>,
    pub cpu_load_1m: Option<f64>,
    pub cpu_temperature_c: Option<f64>,
    pub memory: Option<Memory>,
    pub battery: Option<Battery>,
    pub disks: Vec<Disk>,
    pub network: Network,
    pub focused_window: Option<String>,
    pub running_applications: Vec<String>,
    pub pending_updates: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Gpu {
    pub name: String,
    pub utilisation_percent: u32,
    pub memory_used_mb: u64,
    pub memory_total_mb: u64,
    pub temperature_c: Option<u32>,
    pub power_draw_w: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Memory {
    pub total_mb: u64,
    pub available_mb: u64,
}

impl Memory {
    /// How close to full, as a fraction.
    pub fn pressure(&self) -> f64 {
        if self.total_mb == 0 {
            0.0
        } else {
            1.0 - (self.available_mb as f64 / self.total_mb as f64)
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Battery {
    pub percent: Option<u32>,
    pub on_ac: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Disk {
    pub mount: String,
    pub total_gb: f64,
    pub free_gb: f64,
}

impl Disk {
    pub fn used_percent(&self) -> f64 {
        if self.total_gb <= 0.0 {
            0.0
        } else {
            (1.0 - self.free_gb / self.total_gb) * 100.0
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Network {
    pub online: bool,
    /// True when the connection is known to be metered. Unknown reads as false,
    /// because treating every connection as metered would stop all useful work.
    pub metered: bool,
    pub interface: Option<String>,
}

impl MachineState {
    pub fn read() -> Self {
        Self {
            gpu: read_gpu(),
            cpu_load_1m: read_load(),
            cpu_temperature_c: read_cpu_temperature(),
            memory: read_memory(),
            battery: read_battery(),
            disks: read_disks(),
            network: read_network(),
            focused_window: read_focused_window(),
            running_applications: Vec::new(),
            pending_updates: read_pending_updates(),
        }
    }

    /// A sentence the model can read, rather than a wall of JSON.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(gpu) = &self.gpu {
            parts.push(format!(
                "{} at {}% with {}MB of {}MB used",
                gpu.name, gpu.utilisation_percent, gpu.memory_used_mb, gpu.memory_total_mb
            ));
        }
        if let Some(load) = self.cpu_load_1m {
            parts.push(format!("load {:.2}", load));
        }
        if let Some(memory) = &self.memory {
            parts.push(format!("memory {:.0}% used", memory.pressure() * 100.0));
        }
        if let Some(battery) = &self.battery {
            parts.push(match battery.percent {
                Some(percent) if battery.on_ac => format!("battery {}%, on mains", percent),
                Some(percent) => format!("battery {}%, unplugged", percent),
                None if battery.on_ac => "on mains".to_string(),
                None => "on battery".to_string(),
            });
        }
        for disk in &self.disks {
            if disk.used_percent() > 85.0 {
                parts.push(format!("{} is {:.0}% full", disk.mount, disk.used_percent()));
            }
        }
        if !self.network.online {
            parts.push("offline".to_string());
        } else if self.network.metered {
            parts.push("on a metered connection".to_string());
        }
        if parts.is_empty() {
            "nothing readable about this machine".to_string()
        } else {
            parts.join(", ")
        }
    }
}

fn read_gpu() -> Option<Gpu> {
    let output = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?;
    let fields: Vec<&str> = line.split(',').map(str::trim).collect();
    if fields.len() < 4 {
        return None;
    }
    Some(Gpu {
        name: fields[0].to_string(),
        utilisation_percent: fields[1].parse().unwrap_or(0),
        memory_used_mb: fields[2].parse().unwrap_or(0),
        memory_total_mb: fields[3].parse().unwrap_or(0),
        temperature_c: fields.get(4).and_then(|v| v.parse().ok()),
        power_draw_w: fields.get(5).and_then(|v| v.parse().ok()),
    })
}

fn read_load() -> Option<f64> {
    let text = std::fs::read_to_string("/proc/loadavg").ok()?;
    text.split_whitespace().next()?.parse().ok()
}

fn read_cpu_temperature() -> Option<f64> {
    let entries = std::fs::read_dir("/sys/class/thermal").ok()?;
    for entry in entries.flatten() {
        let kind = std::fs::read_to_string(entry.path().join("type")).unwrap_or_default();
        if !kind.contains("x86_pkg_temp") && !kind.contains("cpu") && !kind.contains("acpitz") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(entry.path().join("temp")) {
            if let Ok(millidegrees) = text.trim().parse::<f64>() {
                return Some(millidegrees / 1000.0);
            }
        }
    }
    None
}

fn read_memory() -> Option<Memory> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut total = 0u64;
    let mut available = 0u64;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("MemTotal:") => total = fields.next()?.parse().ok()?,
            Some("MemAvailable:") => available = fields.next()?.parse().ok()?,
            _ => {}
        }
    }
    if total == 0 {
        return None;
    }
    Some(Memory {
        total_mb: total / 1024,
        available_mb: available / 1024,
    })
}

fn read_battery() -> Option<Battery> {
    let entries = std::fs::read_dir("/sys/class/power_supply").ok()?;
    let mut percent = None;
    let mut on_ac = None;
    for entry in entries.flatten() {
        let kind = std::fs::read_to_string(entry.path().join("type")).unwrap_or_default();
        match kind.trim() {
            "Battery" => {
                percent = std::fs::read_to_string(entry.path().join("capacity"))
                    .ok()
                    .and_then(|text| text.trim().parse().ok());
            }
            "Mains" => {
                let online = std::fs::read_to_string(entry.path().join("online"))
                    .map(|text| text.trim() == "1")
                    .unwrap_or(false);
                on_ac = Some(on_ac.unwrap_or(false) || online);
            }
            _ => {}
        }
    }
    match (percent, on_ac) {
        (None, None) => None,
        // A desktop has no battery and is always on mains.
        (percent, on_ac) => Some(Battery {
            percent,
            on_ac: on_ac.unwrap_or(true),
        }),
    }
}

fn read_disks() -> Vec<Disk> {
    let mut disks = Vec::new();
    let Ok(output) = std::process::Command::new("df")
        .args(["-B1", "--output=target,size,avail", "-x", "tmpfs", "-x", "devtmpfs"])
        .output()
    else {
        return disks;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 3 {
            continue;
        }
        let (Ok(total), Ok(free)) = (fields[1].parse::<u64>(), fields[2].parse::<u64>()) else {
            continue;
        };
        // Only real mounts worth reporting.
        if total < 1_000_000_000 {
            continue;
        }
        disks.push(Disk {
            mount: fields[0].to_string(),
            total_gb: total as f64 / 1e9,
            free_gb: free as f64 / 1e9,
        });
    }
    disks
}

fn read_network() -> Network {
    // A default route is the cheapest honest test of being online.
    let online = std::fs::read_to_string("/proc/net/route")
        .map(|text| {
            text.lines()
                .skip(1)
                .any(|line| line.split_whitespace().nth(1) == Some("00000000"))
        })
        .unwrap_or(false);

    let interface = std::fs::read_to_string("/proc/net/route").ok().and_then(|text| {
        text.lines()
            .skip(1)
            .find(|line| line.split_whitespace().nth(1) == Some("00000000"))
            .and_then(|line| line.split_whitespace().next().map(str::to_string))
    });

    // NetworkManager knows whether a connection is metered; without it the
    // honest answer is that we do not know, which reads as not metered.
    let metered = std::process::Command::new("nmcli")
        .args(["-t", "-f", "GENERAL.METERED", "device", "show"])
        .output()
        .ok()
        .map(|output| {
            let text = String::from_utf8_lossy(&output.stdout).to_lowercase();
            text.contains("yes")
        })
        .unwrap_or(false);

    Network {
        online,
        metered,
        interface,
    }
}

fn read_focused_window() -> Option<String> {
    let output = std::process::Command::new("hyprctl")
        .args(["activewindow", "-j"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    value
        .get("title")
        .and_then(|title| title.as_str())
        .map(str::to_string)
}

fn read_pending_updates() -> Option<u32> {
    let output = std::process::Command::new("checkupdates").output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).lines().count() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_the_machine_never_panics() {
        let state = MachineState::read();
        let _ = state.summary();
    }

    #[test]
    fn a_summary_is_always_a_sentence() {
        let summary = MachineState::default().summary();
        assert!(!summary.is_empty());
    }

    #[test]
    fn memory_pressure_is_a_fraction() {
        let memory = Memory {
            total_mb: 32_000,
            available_mb: 8_000,
        };
        assert!((memory.pressure() - 0.75).abs() < 1e-9);
    }

    #[test]
    fn empty_memory_does_not_divide_by_zero() {
        let memory = Memory {
            total_mb: 0,
            available_mb: 0,
        };
        assert_eq!(memory.pressure(), 0.0);
    }

    #[test]
    fn disk_usage_is_a_percentage() {
        let disk = Disk {
            mount: "/".to_string(),
            total_gb: 100.0,
            free_gb: 10.0,
        };
        assert!((disk.used_percent() - 90.0).abs() < 1e-9);
    }

    #[test]
    fn a_full_disk_is_mentioned_in_the_summary() {
        let state = MachineState {
            disks: vec![Disk {
                mount: "/var".to_string(),
                total_gb: 100.0,
                free_gb: 5.0,
            }],
            ..Default::default()
        };
        assert!(state.summary().contains("/var"), "{}", state.summary());
    }

    #[test]
    fn a_metered_connection_is_mentioned_so_the_model_can_defer() {
        let state = MachineState {
            network: Network {
                online: true,
                metered: true,
                interface: None,
            },
            ..Default::default()
        };
        assert!(state.summary().contains("metered"), "{}", state.summary());
    }

    #[test]
    fn being_offline_is_mentioned() {
        let state = MachineState::default();
        assert!(state.summary().contains("offline"), "{}", state.summary());
    }

    #[test]
    fn a_missing_reading_stays_missing_rather_than_becoming_zero() {
        let state = MachineState::default();
        assert!(state.gpu.is_none());
        assert!(state.cpu_load_1m.is_none());
        assert!(state.memory.is_none());
    }
}
