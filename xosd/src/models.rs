//! The model catalogue.
//!
//! `verified-models.json` says which local model suits which machine. The
//! install script reads it while the daemon does not yet exist; this reads it
//! afterwards, for the wizard and for anything else that needs to know what
//! this machine should be running.
//!
//! Both read the same file and apply the same rule, and that rule is written
//! down in one place here so the two answers cannot drift into disagreeing
//! about what a machine can run.
//!
//! # On thresholds
//!
//! Every minimum in the catalogue is written against what a machine reports,
//! not what it was sold as. A machine sold as 8GB reports something near 7800
//! MB, because firmware and an integrated GPU take their cut before Linux sees
//! any of it. A threshold of 8192 would exclude every 8GB machine, and XOS says
//! in its own specification that 8GB machines are the target.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::hardware::{Inventory, Profile};

/// The catalogue format this build understands. Same rule as the hardware
/// table: a file written for a later format may mean something different by the
/// same field names, and reading it anyway would hand somebody the wrong model.
const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
pub struct Catalogue {
    // The version is checked at load and never kept, since a field here could
    // only ever hold FORMAT_VERSION.
    #[serde(default)]
    pub updated: String,
    #[serde(default)]
    pub entries: Vec<Model>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub repository: String,
    pub file: String,
    #[serde(default)]
    pub quantisation: String,
    #[serde(default)]
    pub approximate_size_gb: f64,
    #[serde(default)]
    pub minimum_vram_mb: u64,
    #[serde(default)]
    pub minimum_memory_mb: u64,
    #[serde(default)]
    pub profiles: Vec<String>,
    #[serde(default = "lowest_rank")]
    pub rank: u32,
    #[serde(default)]
    pub confidence: String,
    #[serde(default)]
    pub why: Option<String>,
    #[serde(default)]
    pub context: Option<u32>,
}

fn lowest_rank() -> u32 {
    99
}

/// What this machine should run, and why not the others.
#[derive(Debug, Clone, Serialize)]
pub struct Recommendation {
    #[serde(flatten)]
    pub model: Option<Model>,
    /// The machine, so the wizard can show the fit rather than assert it.
    pub available_vram_mb: u64,
    pub memory_mb: u64,
    pub profile: String,
    /// Why there is no recommendation, when there is none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The ones that did not fit, and what stopped them. Somebody wondering why
    /// they were not offered the big model deserves the answer.
    pub rejected: Vec<Rejected>,
    /// When the catalogue was last updated, so somebody can tell whether they
    /// are being offered something chosen two years ago.
    pub catalogue_updated: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Rejected {
    pub name: String,
    pub because: String,
}

impl Catalogue {
    pub fn load() -> Result<Self, String> {
        let path = Self::path().ok_or_else(|| "cannot find verified-models.json".to_string())?;
        Self::load_from(&path)
    }

    pub fn load_from(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;

        // Read before the rest of the file, so a later format is refused with
        // the reason that explains it rather than whatever failed to parse.
        let loose: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| format!("{} is not valid JSON: {}", path.display(), e))?;
        let version = loose
            .get("format_version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        if version != FORMAT_VERSION {
            return Err(format!(
                "{} is format {}, and this XOS reads format {}",
                path.display(),
                version,
                FORMAT_VERSION
            ));
        }

        serde_json::from_str(&text).map_err(|e| format!("{} is not valid: {}", path.display(), e))
    }

    fn path() -> Option<PathBuf> {
        let mut candidates = Vec::new();
        if let Ok(explicit) = std::env::var("XOS_MODELS_DB") {
            candidates.push(PathBuf::from(explicit));
        }
        if let Some(data) = dirs::data_dir() {
            candidates.push(data.join("xos/verified-models.json"));
        }
        candidates.push(PathBuf::from("/usr/share/xos/verified-models.json"));
        if let Ok(exe) = std::env::current_exe() {
            let mut directory = exe.parent().map(Path::to_path_buf);
            while let Some(current) = directory {
                candidates.push(current.join("verified-models.json"));
                directory = current.parent().map(Path::to_path_buf);
            }
        }
        candidates.into_iter().find(|path| path.exists())
    }

    /// The best model this machine can actually hold.
    pub fn recommend(&self, inventory: &Inventory) -> Recommendation {
        let profile = inventory.profile();
        let vram = inventory
            .gpus
            .iter()
            .filter_map(|gpu| gpu.vram_mb)
            .max()
            .unwrap_or(0);
        let memory = inventory.memory.total_mb;
        let label = profile.label().to_string();

        if profile == Profile::ApiOnly {
            // Pulling gigabytes onto a machine that cannot run them wastes
            // somebody's bandwidth and teaches them nothing true about local
            // models.
            return Recommendation {
                model: None,
                available_vram_mb: vram,
                memory_mb: memory,
                profile: label,
                reason: Some(
                    "this machine has neither a usable GPU nor the memory for a \
                     small model, so XOS will work through an API"
                        .to_string(),
                ),
                rejected: Vec::new(),
                catalogue_updated: self.updated.clone(),
            };
        }

        let mut fits = Vec::new();
        let mut rejected = Vec::new();

        for model in &self.entries {
            if !model.profiles.iter().any(|p| p == &label) {
                rejected.push(Rejected {
                    name: model.name.clone(),
                    because: format!("it is not offered for a {} machine", label),
                });
                continue;
            }
            if profile == Profile::Local && vram < model.minimum_vram_mb {
                rejected.push(Rejected {
                    name: model.name.clone(),
                    because: format!(
                        "it wants {} MB of VRAM and this machine has {}",
                        model.minimum_vram_mb, vram
                    ),
                });
                continue;
            }
            if memory < model.minimum_memory_mb {
                rejected.push(Rejected {
                    name: model.name.clone(),
                    because: format!(
                        "it wants {} MB of memory and this machine has {}",
                        model.minimum_memory_mb, memory
                    ),
                });
                continue;
            }
            fits.push(model.clone());
        }

        // Rank first, and a confirmed row beats an unproven one at the same
        // rank: somebody has actually run it.
        fits.sort_by(|a, b| {
            a.rank.cmp(&b.rank).then_with(|| {
                let weight = |m: &Model| if m.confidence == "confirmed" { 0 } else { 1 };
                weight(a).cmp(&weight(b))
            })
        });

        let reason = if fits.is_empty() {
            Some("nothing in the catalogue fits this machine".to_string())
        } else {
            None
        };

        Recommendation {
            model: fits.into_iter().next(),
            available_vram_mb: vram,
            memory_mb: memory,
            profile: label,
            reason,
            rejected,
            catalogue_updated: self.updated.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::{Cpu, Device, Firmware, MemoryInfo};

    fn catalogue() -> Catalogue {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("verified-models.json");
        Catalogue::load_from(&path).expect("the shipped catalogue must parse")
    }

    fn machine(vram: Option<u64>, memory_mb: u64) -> Inventory {
        Inventory {
            cpu: Cpu {
                vendor: "test".to_string(),
                model: "test".to_string(),
                cores: 4,
                threads: 8,
                microarchitecture_level: 2,
                baseline_ok: true,
                notable_flags: Vec::new(),
            },
            memory: MemoryInfo {
                total_mb: memory_mb,
                available_mb: memory_mb / 2,
            },
            gpus: vram
                .map(|vram| {
                    vec![Device {
                        bus: "pci".to_string(),
                        slot: "01:00.0".to_string(),
                        class: "display".to_string(),
                        description: "a card".to_string(),
                        vendor_id: "10de".to_string(),
                        device_id: "1b80".to_string(),
                        subsystem_id: None,
                        kernel_driver: None,
                        vram_mb: Some(vram),
                    }]
                })
                .unwrap_or_default(),
            network: Vec::new(),
            audio: Vec::new(),
            bluetooth: Vec::new(),
            storage: Vec::new(),
            firmware: Firmware {
                mode: "uefi".to_string(),
                secure_boot: None,
            },
            displays: Vec::new(),
        }
    }

    #[test]
    fn the_reference_machine_gets_the_reference_model() {
        // A GTX 1080 with 8GB of VRAM and 16GB of memory. This is the machine
        // XOS is built around, and Gemma 4 E4B is what was measured on it.
        let recommendation = catalogue().recommend(&machine(Some(8192), 16000));
        let model = recommendation.model.expect("a model");
        assert_eq!(model.id, "gemma-4-e4b");
        assert_eq!(model.confidence, "confirmed");
    }

    #[test]
    fn a_real_8gb_machine_is_not_excluded_by_a_rounding_error() {
        // A machine sold as 8GB reports about 7800 MB. Every threshold written
        // as 8192 would exclude the exact machine XOS exists for.
        let recommendation = catalogue().recommend(&machine(Some(4096), 7800));
        assert!(
            recommendation.model.is_some(),
            "an 8GB machine was offered nothing: {:?}",
            recommendation.reason
        );
    }

    #[test]
    fn a_machine_with_no_gpu_gets_something_it_can_run_on_the_cpu() {
        let recommendation = catalogue().recommend(&machine(None, 7800));
        let model = recommendation.model.expect("a model");
        assert!(model.profiles.iter().any(|p| p == "cpu"));
    }

    #[test]
    fn an_api_only_machine_is_offered_nothing_and_told_why() {
        let recommendation = catalogue().recommend(&machine(None, 2048));
        assert!(recommendation.model.is_none());
        assert!(recommendation
            .reason
            .expect("a reason")
            .contains("work through an API"));
    }

    #[test]
    fn what_did_not_fit_is_reported_with_the_numbers() {
        // Somebody wondering why they were not offered the big model deserves
        // the answer rather than silence.
        let recommendation = catalogue().recommend(&machine(Some(4096), 7800));
        let big = recommendation
            .rejected
            .iter()
            .find(|r| r.name.contains("Qwen3 8B"))
            .expect("the big model was not mentioned at all");
        assert!(big.because.contains("VRAM") || big.because.contains("memory"));
        assert!(
            big.because.contains("4096") || big.because.contains("7800"),
            "{}",
            big.because
        );
    }

    #[test]
    fn the_recommendation_carries_the_machine_so_the_fit_can_be_shown() {
        let recommendation = catalogue().recommend(&machine(Some(8192), 16000));
        assert_eq!(recommendation.available_vram_mb, 8192);
        assert_eq!(recommendation.memory_mb, 16000);
        assert_eq!(recommendation.profile, "local");
    }

    #[test]
    fn the_catalogue_says_when_it_was_last_looked_at() {
        // Somebody should be able to tell whether they are being offered
        // something chosen two years ago.
        let recommendation = catalogue().recommend(&machine(Some(8192), 16000));
        assert!(!recommendation.catalogue_updated.is_empty());
    }

    #[test]
    fn a_catalogue_from_a_later_format_is_refused_rather_than_misread() {
        let mut path = std::env::temp_dir();
        path.push("xos-models-format-test.json");
        std::fs::write(&path, r#"{"format_version":99,"entries":[]}"#).expect("write");
        let outcome = Catalogue::load_from(&path);
        assert!(outcome.is_err());
        assert!(outcome.unwrap_err().contains("format 99"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn no_shipped_threshold_is_a_round_power_of_two() {
        // The mistake that excluded every 8GB machine, pinned so it cannot come
        // back when somebody adds a row.
        for model in catalogue().entries {
            for (field, value) in [
                ("minimum_memory_mb", model.minimum_memory_mb),
                ("minimum_vram_mb", model.minimum_vram_mb),
            ] {
                assert!(
                    ![1024u64, 2048, 4096, 8192, 16384, 32768].contains(&value),
                    "{} has {} = {}, which excludes the machine it was written for",
                    model.name,
                    field,
                    value
                );
            }
        }
    }
}
