//! Execution conditions checked against system state.
//!
//! A node can say it wants AC power, an idle GPU, or an unmetered connection. A
//! condition that is not met holds the node back rather than failing it: waiting
//! for the laptop to be plugged in is not an error, and treating it as one would
//! burn the node's retry budget on something that was never wrong.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Condition {
    OnAcPower,
    GpuIdle,
    UnmeteredNetwork,
}

impl Condition {
    pub fn label(&self) -> &'static str {
        match self {
            Condition::OnAcPower => "on-ac-power",
            Condition::GpuIdle => "gpu-idle",
            Condition::UnmeteredNetwork => "unmetered-network",
        }
    }

    pub fn met(&self, state: &SystemState) -> bool {
        match self {
            Condition::OnAcPower => state.on_ac_power,
            Condition::GpuIdle => state.gpu_idle,
            Condition::UnmeteredNetwork => state.unmetered_network,
        }
    }
}

/// What the machine looks like right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemState {
    pub on_ac_power: bool,
    pub gpu_idle: bool,
    pub unmetered_network: bool,
}

impl SystemState {
    /// Everything permitted. The right default for a desktop, and for tests.
    pub fn unrestricted() -> Self {
        Self {
            on_ac_power: true,
            gpu_idle: true,
            unmetered_network: true,
        }
    }

    /// Read the machine.
    ///
    /// Anything that cannot be determined is treated as permitting work. A
    /// laptop with no battery reports no power supply, and refusing to work
    /// there would be worse than occasionally running on battery.
    pub fn read() -> Self {
        Self {
            on_ac_power: read_ac_power().unwrap_or(true),
            gpu_idle: read_gpu_idle().unwrap_or(true),
            // Offline or metered both mean "do not do this now". Read from the
            // same place the model reads it, so the two never disagree.
            unmetered_network: {
                let network = crate::scheduler::machine::MachineState::read().network;
                network.online && !network.metered
            },
        }
    }
}

/// `/sys/class/power_supply/*/online` is 1 when the mains adapter is connected.
fn read_ac_power() -> Option<bool> {
    let entries = std::fs::read_dir("/sys/class/power_supply").ok()?;
    let mut saw_mains = false;
    for entry in entries.flatten() {
        let kind = std::fs::read_to_string(entry.path().join("type")).unwrap_or_default();
        if kind.trim() != "Mains" {
            continue;
        }
        saw_mains = true;
        if std::fs::read_to_string(entry.path().join("online"))
            .map(|text| text.trim() == "1")
            .unwrap_or(false)
        {
            return Some(true);
        }
    }
    if saw_mains {
        Some(false)
    } else {
        // A desktop has no mains supply to read, and is always on mains.
        None
    }
}

/// Below this, the GPU is doing nothing worth protecting.
const GPU_IDLE_PERCENT: u32 = 20;

fn read_gpu_idle() -> Option<bool> {
    let output = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=utilization.gpu", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut highest = 0u32;
    let mut seen = false;
    for line in text.lines() {
        if let Ok(value) = line.trim().parse::<u32>() {
            highest = highest.max(value);
            seen = true;
        }
    }
    if seen {
        Some(highest < GPU_IDLE_PERCENT)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrestricted_meets_everything() {
        let state = SystemState::unrestricted();
        for condition in [
            Condition::OnAcPower,
            Condition::GpuIdle,
            Condition::UnmeteredNetwork,
        ] {
            assert!(condition.met(&state), "{} should be met", condition.label());
        }
    }

    #[test]
    fn battery_fails_only_the_power_condition() {
        let state = SystemState {
            on_ac_power: false,
            ..SystemState::unrestricted()
        };
        assert!(!Condition::OnAcPower.met(&state));
        assert!(Condition::GpuIdle.met(&state));
    }

    #[test]
    fn a_busy_gpu_fails_only_the_gpu_condition() {
        let state = SystemState {
            gpu_idle: false,
            ..SystemState::unrestricted()
        };
        assert!(!Condition::GpuIdle.met(&state));
        assert!(Condition::OnAcPower.met(&state));
    }

    #[test]
    fn conditions_round_trip_through_json() {
        let conditions = vec![Condition::OnAcPower, Condition::GpuIdle];
        let text = serde_json::to_string(&conditions).expect("encode");
        assert!(text.contains("on-ac-power"), "{}", text);
        let back: Vec<Condition> = serde_json::from_str(&text).expect("decode");
        assert_eq!(back, conditions);
    }

    #[test]
    fn reading_the_machine_never_panics() {
        // Whatever this box is, it must answer.
        let state = SystemState::read();
        let _ = state.on_ac_power && state.gpu_idle;
    }
}
