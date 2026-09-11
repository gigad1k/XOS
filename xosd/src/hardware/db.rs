//! The community hardware database.
//!
//! Resolution is a lookup, not a guess. `hardware-db.json` holds the table, and
//! this module reads it and answers one question: given a device, which driver
//! should the installer reach for, and what does it fall back to when that one
//! does not work?
//!
//! Keeping the table in a file rather than in code is the point. The set of
//! machines XOS runs on is larger than anyone can test, so the table has to be
//! something a stranger can add a row to. That is also why every row carries a
//! confidence and, where it matters, a `why`: someone reading it later needs to
//! know whether a row was confirmed on real hardware or inferred from documents.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::Device;

/// The table format this build understands.
const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
pub struct Database {
    // The format version is not kept: `load_from` refuses anything but
    // FORMAT_VERSION, so a field here could only ever hold that constant.
    #[serde(default)]
    pub updated: String,
    pub nvidia_branches: NvidiaBranches,
    pub amd_generations: AmdGenerations,
    #[serde(default)]
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AmdGenerations {
    pub ranges: Vec<AmdRange>,
    pub generation_parameters: std::collections::HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AmdRange {
    pub from: String,
    pub to: String,
    pub generation: String,
    #[serde(default)]
    pub codename: String,
    #[serde(default)]
    pub confidence: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NvidiaBranches {
    pub ranges: Vec<Range>,
    pub branch_packages: std::collections::HashMap<String, BranchPackages>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Range {
    pub from: String,
    pub to: String,
    pub architecture: String,
    pub branch: String,
    #[serde(default)]
    pub confidence: String,
    #[serde(default)]
    pub why: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BranchPackages {
    pub driver: String,
    pub utils: String,
    #[serde(default)]
    pub kernel_parameters: Vec<String>,
    #[serde(default)]
    pub why: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    pub id: String,
    pub class: String,
    pub vendor: String,
    #[serde(default)]
    pub resolve: Option<String>,
    #[serde(default)]
    pub driver: Option<String>,
    #[serde(default)]
    pub firmware: Vec<String>,
    #[serde(default)]
    pub kernel_parameters: Vec<String>,
    #[serde(default)]
    pub fallback: Vec<String>,
    #[serde(default)]
    pub confidence: String,
    #[serde(default)]
    pub why: Option<String>,
}

/// What the installer should do about one device.
#[derive(Debug, Clone, Serialize)]
pub struct Resolution {
    pub device: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    pub vendor_id: String,
    pub device_id: String,
    pub class: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    /// The package to install, or "in-tree" when the kernel already has it.
    pub driver: String,
    /// The userspace that goes with it, where the driver is only half the
    /// story. A kernel module without its libGL and Xorg driver produces a
    /// machine that boots to a black screen, so this is not optional and the
    /// installer treats its failure as the driver's failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub utils: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub firmware: Vec<String>,
    pub kernel_parameters: Vec<String>,
    /// What to try next, in order, if the recommendation does not work.
    ///
    /// The chain is the whole reason this is worth doing: an unbootable desktop
    /// after an install is the single worst outcome, and every entry here is a
    /// way back to a working screen.
    pub fallback: Vec<String>,
    pub confidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kernel_driver_in_use: Option<String>,
}

impl Database {
    /// Find and read the table.
    pub fn load() -> Result<Self, String> {
        let path = Self::path().ok_or_else(|| "cannot find hardware-db.json".to_string())?;
        Self::load_from(&path)
    }

    pub fn load_from(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;

        // The version is read before the rest of the file, not after. A table
        // written for a later format may mean something different by the same
        // field names, and it will very likely also fail to parse — at which
        // point the reason given would be a missing field rather than the real
        // one. Whoever is looking at a machine with the wrong driver deserves
        // the actual answer.
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

    /// The table ships with XOS, but a machine may carry a newer one, so a
    /// locally updated copy wins over the one beside the binary.
    fn path() -> Option<PathBuf> {
        let mut candidates = Vec::new();
        if let Ok(explicit) = std::env::var("XOS_HARDWARE_DB") {
            candidates.push(PathBuf::from(explicit));
        }
        if let Some(data) = dirs::data_dir() {
            candidates.push(data.join("xos/hardware-db.json"));
        }
        candidates.push(PathBuf::from("/usr/share/xos/hardware-db.json"));
        if let Ok(exe) = std::env::current_exe() {
            // Walk up from the binary so a repository checkout works too.
            let mut directory = exe.parent().map(Path::to_path_buf);
            while let Some(current) = directory {
                candidates.push(current.join("hardware-db.json"));
                directory = current.parent().map(Path::to_path_buf);
            }
        }
        candidates.into_iter().find(|path| path.exists())
    }

    /// Which architecture and driver branch an NVIDIA device ID belongs to.
    ///
    /// Ranges are tested in file order because they overlap: the Volta IDs sit
    /// inside the Pascal range, so Volta is listed first and wins. Sorting them
    /// here would silently undo that.
    pub fn nvidia_range(&self, device_id: &str) -> Option<&Range> {
        let id = parse_hex(device_id)?;
        self.nvidia_branches.ranges.iter().find(|range| {
            match (parse_hex(&range.from), parse_hex(&range.to)) {
                (Some(from), Some(to)) => id >= from && id <= to,
                _ => false,
            }
        })
    }

    /// The row for a device.
    ///
    /// A row is keyed `pci:<vendor>`, optionally with a qualifier after a
    /// second colon (`pci:8086:wifi`) where one vendor needs more than one row
    /// for the same class. Both forms match here so adding a qualified row
    /// never silently stops the plain one from being found.
    /// Which AMD generation a device ID belongs to, where it matters.
    pub fn amd_generation(&self, device_id: &str) -> Option<&AmdRange> {
        let id = parse_hex(device_id)?;
        self.amd_generations.ranges.iter().find(|range| {
            match (parse_hex(&range.from), parse_hex(&range.to)) {
                (Some(from), Some(to)) => id >= from && id <= to,
                _ => false,
            }
        })
    }

    fn entry(&self, vendor_id: &str, class: &str) -> Option<&Entry> {
        let exact = format!("pci:{}", vendor_id);
        let qualified = format!("pci:{}:", vendor_id);
        self.entries.iter().find(|entry| {
            entry.class == class
                && (entry.id == exact || entry.id.starts_with(&qualified))
        })
    }

    /// Resolve one device to a driver and a fallback chain.
    pub fn resolve(&self, device: &Device) -> Resolution {
        let entry = self.entry(&device.vendor_id, &device.class);

        // Some vendors cannot be answered with one package name. The row says
        // so, by naming the table to consult, rather than this code carrying a
        // hardcoded list of which vendors are special.
        if entry.and_then(|entry| entry.resolve.as_deref()) == Some("nvidia_branches") {
            if let Some(range) = self.nvidia_range(&device.device_id) {
                let packages = self.nvidia_branches.branch_packages.get(&range.branch);

                // Exactly one proprietary branch drives any given card. The
                // others do not partly work on it, they do not work at all, so
                // the chain is the open driver and then a plain framebuffer —
                // whatever the row says, ending somewhere that always produces
                // a picture.
                let mut fallback: Vec<String> = entry
                    .map(|entry| entry.fallback.clone())
                    .unwrap_or_default();
                if fallback.is_empty() {
                    fallback.push("nouveau".to_string());
                }
                if fallback.last().map(String::as_str) != Some("vesa") {
                    fallback.push("vesa".to_string());
                }

                return Resolution {
                    device: device.description.clone(),
                    vendor: entry.map(|entry| entry.vendor.clone()),
                    vendor_id: device.vendor_id.clone(),
                    device_id: device.device_id.clone(),
                    class: device.class.clone(),
                    architecture: Some(range.architecture.clone()),
                    driver: packages
                        .map(|p| p.driver.clone())
                        .unwrap_or_else(|| "nouveau".to_string()),
                    utils: packages.map(|p| p.utils.clone()),
                    branch: Some(range.branch.clone()),
                    firmware: vec!["linux-firmware".to_string()],
                    kernel_parameters: packages
                        .map(|p| p.kernel_parameters.clone())
                        .unwrap_or_default(),
                    fallback,
                    confidence: range.confidence.clone(),
                    why: range
                        .why
                        .clone()
                        .or_else(|| packages.and_then(|p| p.why.clone())),
                    kernel_driver_in_use: device.kernel_driver.clone(),
                };
            }
        }

        // GCN 1.0 and 1.1 are driven by radeon unless the kernel is told
        // otherwise, and a card that falls to radeon silently is the kind of
        // fault nobody can see: the machine is just slow. The parameters belong
        // on the recommendation, not on a fallback path nobody reaches.
        let mut amd_parameters: Vec<String> = Vec::new();
        let mut amd_architecture: Option<String> = None;
        let mut amd_confidence: Option<String> = None;
        if device.vendor_id == "1002" && device.class == "display" {
            if let Some(range) = self.amd_generation(&device.device_id) {
                if let Some(parameters) = self
                    .amd_generations
                    .generation_parameters
                    .get(&range.generation)
                {
                    amd_parameters = parameters.clone();
                }
                // Named, because otherwise the kernel parameters beside it look
                // arbitrary and the next person deletes them.
                amd_architecture = Some(if range.codename.is_empty() {
                    range.generation.clone()
                } else {
                    format!("{} ({})", range.generation, range.codename)
                });
                if !range.confidence.is_empty() {
                    amd_confidence = Some(range.confidence.clone());
                }
            }
        }

        match entry {
            Some(entry) => Resolution {
                device: device.description.clone(),
                vendor: Some(entry.vendor.clone()),
                vendor_id: device.vendor_id.clone(),
                device_id: device.device_id.clone(),
                class: device.class.clone(),
                architecture: amd_architecture,
                driver: entry
                    .driver
                    .clone()
                    .unwrap_or_else(|| "in-tree".to_string()),
                utils: None,
                branch: None,
                firmware: entry.firmware.clone(),
                kernel_parameters: if amd_parameters.is_empty() {
                    entry.kernel_parameters.clone()
                } else {
                    amd_parameters
                },
                fallback: entry.fallback.clone(),
                confidence: amd_confidence.unwrap_or_else(|| entry.confidence.clone()),
                why: entry.why.clone(),
                kernel_driver_in_use: device.kernel_driver.clone(),
            },
            // An unknown device is reported as unknown. Naming a driver XOS has
            // no reason to believe in is how people end up without a screen.
            //
            // In particular, not the module currently driving it: that is a
            // fact about this machine, reported separately, and it is very often
            // not something anyone can install. HW-2 would take it as a package
            // name and fail on it.
            None => Resolution {
                device: device.description.clone(),
                vendor: None,
                vendor_id: device.vendor_id.clone(),
                device_id: device.device_id.clone(),
                class: device.class.clone(),
                architecture: None,
                driver: "unknown".to_string(),
                utils: None,
                branch: None,
                firmware: Vec::new(),
                kernel_parameters: Vec::new(),
                fallback: if device.class == "display" {
                    vec!["vesa".to_string()]
                } else {
                    Vec::new()
                },
                confidence: "unknown".to_string(),
                why: Some("not in the database; add a row if you get this working".to_string()),
                kernel_driver_in_use: device.kernel_driver.clone(),
            },
        }
    }
}

fn parse_hex(text: &str) -> Option<u32> {
    u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Database {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("hardware-db.json");
        Database::load_from(&root).expect("the shipped database must parse")
    }

    fn gpu(device_id: &str) -> Device {
        Device {
            bus: "pci".to_string(),
            slot: "01:00.0".to_string(),
            class: "display".to_string(),
            description: "test card".to_string(),
            vendor_id: "10de".to_string(),
            device_id: device_id.to_string(),
            subsystem_id: None,
            kernel_driver: None,
            vram_mb: None,
        }
    }

    #[test]
    fn a_gtx_1080_is_pascal_on_the_580_branch() {
        // The reference machine. If this ever stops holding, the check HW-1 was
        // written against has stopped being true.
        let resolution = database().resolve(&gpu("1b80"));
        assert_eq!(resolution.architecture.as_deref(), Some("Pascal"));
        assert_eq!(resolution.branch.as_deref(), Some("580"));
        assert_eq!(resolution.driver, "nvidia-580xx-dkms");
    }

    #[test]
    fn a_kernel_module_never_arrives_without_its_userspace() {
        // A driver with no libGL and no Xorg driver is a black screen, which is
        // the exact outcome the install step exists to prevent.
        for id in ["1b80", "1183", "0dc4", "2684"] {
            let resolution = database().resolve(&gpu(id));
            assert!(
                resolution.utils.is_some(),
                "{} installs a kernel module with no userspace",
                id
            );
        }
        assert_eq!(
            database().resolve(&gpu("1b80")).utils.as_deref(),
            Some("nvidia-580xx-utils")
        );
    }

    #[test]
    fn volta_wins_over_pascal_because_it_is_listed_first() {
        // The Volta IDs sit inside the Pascal range. Order in the file is load
        // bearing, and sorting the ranges would silently break this.
        let resolution = database().resolve(&gpu("1d81"));
        assert_eq!(resolution.architecture.as_deref(), Some("Volta"));
    }

    #[test]
    fn a_kepler_card_gets_the_470_branch() {
        let resolution = database().resolve(&gpu("1183"));
        assert_eq!(resolution.architecture.as_deref(), Some("Kepler"));
        assert_eq!(resolution.branch.as_deref(), Some("470"));
    }

    #[test]
    fn a_fermi_card_gets_the_390_branch() {
        assert_eq!(
            database().resolve(&gpu("0dc4")).branch.as_deref(),
            Some("390")
        );
    }

    #[test]
    fn a_tesla_card_falls_to_nouveau_because_no_branch_still_supports_it() {
        assert_eq!(
            database().resolve(&gpu("05e0")).branch.as_deref(),
            Some("nouveau")
        );
    }

    #[test]
    fn a_modern_card_gets_the_current_branch() {
        let resolution = database().resolve(&gpu("2684"));
        assert_eq!(resolution.architecture.as_deref(), Some("Ada"));
        assert_eq!(resolution.driver, "nvidia-open-dkms");
    }

    #[test]
    fn every_recommendation_carries_a_way_back_to_a_screen() {
        for id in ["1b80", "1183", "0dc4", "2684"] {
            let resolution = database().resolve(&gpu(id));
            assert_eq!(
                resolution.fallback.last().map(String::as_str),
                Some("vesa"),
                "{} has no text-mode fallback, so a bad driver means no screen",
                id
            );
        }
    }

    #[test]
    fn the_chain_never_offers_a_branch_that_cannot_drive_the_card() {
        // One proprietary branch drives any given card; the others do not drive
        // it at all. Offering them to someone whose screen just went black
        // costs them another reboot and gives them a wrong theory about why.
        for id in ["1b80", "1183", "0dc4", "2684", "1d81", "05e0"] {
            let resolution = database().resolve(&gpu(id));
            for step in &resolution.fallback {
                assert!(
                    !step.contains("xx-dkms") && !step.contains("nvidia-open"),
                    "{} offers {} as a fallback, which cannot drive it",
                    id,
                    step
                );
            }
        }
    }

    #[test]
    fn the_chain_ends_somewhere_that_always_produces_a_picture() {
        for id in ["1b80", "2684", "05e0"] {
            let resolution = database().resolve(&gpu(id));
            assert_eq!(
                resolution.fallback,
                vec!["nouveau".to_string(), "vesa".to_string()],
                "{} should fall back to the open driver, then a framebuffer",
                id
            );
        }
    }

    #[test]
    fn an_unknown_card_says_so_rather_than_guessing() {
        let mut device = gpu("ffff");
        device.vendor_id = "dead".to_string();
        let resolution = database().resolve(&device);
        assert_eq!(resolution.confidence, "unknown");
        assert_eq!(resolution.driver, "unknown");
    }

    #[test]
    fn an_unknown_card_does_not_pass_off_its_loaded_module_as_a_package() {
        // The module driving a device now is a fact about this machine, not a
        // thing to install. HW-2 reads `driver` as a package name, and handing
        // it something like `dxgkrnl` fails at the package manager.
        let mut device = gpu("008e");
        device.vendor_id = "1414".to_string();
        device.kernel_driver = Some("dxgkrnl".to_string());
        let resolution = database().resolve(&device);
        assert_eq!(resolution.driver, "unknown");
        assert_eq!(resolution.kernel_driver_in_use.as_deref(), Some("dxgkrnl"));
    }

    #[test]
    fn an_amd_card_resolves_to_mesa() {
        let mut device = gpu("73bf");
        device.vendor_id = "1002".to_string();
        let resolution = database().resolve(&device);
        assert_eq!(resolution.driver, "mesa");
        assert!(resolution.fallback.contains(&"amdgpu".to_string()));
    }

    #[test]
    fn an_old_gcn_card_is_told_to_use_amdgpu() {
        // Tahiti, GCN 1.0. Without the parameter it falls to radeon silently and
        // the machine is slow for a reason nobody can see.
        let mut device = gpu("6798");
        device.vendor_id = "1002".to_string();
        let resolution = database().resolve(&device);
        assert!(
            resolution
                .kernel_parameters
                .contains(&"amdgpu.si_support=1".to_string()),
            "{:?}",
            resolution.kernel_parameters
        );
    }

    #[test]
    fn an_old_amd_card_says_which_generation_it_is() {
        // Without this the kernel parameters beside it look arbitrary, and the
        // next person to read the install log deletes them.
        let mut device = gpu("6798");
        device.vendor_id = "1002".to_string();
        assert_eq!(
            database().resolve(&device).architecture.as_deref(),
            Some("GCN 1.0 (Tahiti)")
        );
    }

    #[test]
    fn a_gcn_1_1_card_gets_the_cik_parameters_not_the_si_ones() {
        let mut device = gpu("67b0");
        device.vendor_id = "1002".to_string();
        let resolution = database().resolve(&device);
        assert!(resolution
            .kernel_parameters
            .contains(&"amdgpu.cik_support=1".to_string()));
        assert!(!resolution
            .kernel_parameters
            .contains(&"amdgpu.si_support=1".to_string()));
    }

    #[test]
    fn a_modern_amd_card_gets_no_legacy_parameters() {
        // The parameters are inert on newer hardware, but putting them on every
        // AMD machine makes the kernel command line noise nobody can read.
        let mut device = gpu("73bf");
        device.vendor_id = "1002".to_string();
        assert!(database().resolve(&device).kernel_parameters.is_empty());
    }

    #[test]
    fn the_amd_ranges_do_not_overlap_each_other() {
        // Unlike the NVIDIA table, order here carries no meaning, so an overlap
        // would be a silent coin toss between two generations.
        let database = database();
        let mut ranges: Vec<(u32, u32, &str)> = database
            .amd_generations
            .ranges
            .iter()
            .map(|r| {
                (
                    parse_hex(&r.from).expect("from"),
                    parse_hex(&r.to).expect("to"),
                    r.codename.as_str(),
                )
            })
            .collect();
        ranges.sort();
        for pair in ranges.windows(2) {
            assert!(
                pair[0].1 < pair[1].0,
                "{} and {} overlap",
                pair[0].2,
                pair[1].2
            );
        }
    }

    #[test]
    fn a_realtek_nic_resolves_to_the_in_tree_driver() {
        let device = Device {
            bus: "pci".to_string(),
            slot: "03:00.0".to_string(),
            class: "network".to_string(),
            description: "RTL8111".to_string(),
            vendor_id: "10ec".to_string(),
            device_id: "8168".to_string(),
            subsystem_id: None,
            kernel_driver: None,
            vram_mb: None,
        };
        let resolution = database().resolve(&device);
        assert_eq!(resolution.driver, "in-tree");
        assert!(resolution.fallback.contains(&"r8168-dkms".to_string()));
    }

    #[test]
    fn the_shipped_table_has_no_range_that_runs_backwards() {
        for range in &database().nvidia_branches.ranges {
            let from = parse_hex(&range.from).expect("a from");
            let to = parse_hex(&range.to).expect("a to");
            assert!(from <= to, "{} runs backwards", range.architecture);
        }
    }

    #[test]
    fn every_branch_named_by_a_range_has_packages() {
        let database = database();
        for range in &database.nvidia_branches.ranges {
            assert!(
                database
                    .nvidia_branches
                    .branch_packages
                    .contains_key(&range.branch),
                "{} names branch {} which has no packages",
                range.architecture,
                range.branch
            );
        }
    }

    #[test]
    fn a_qualified_row_still_matches_its_vendor() {
        // `pci:8086:wifi` exists because Intel needs more than one network row.
        // A plain lookup for 8086 must still find it.
        let device = Device {
            bus: "pci".to_string(),
            slot: "00:14.3".to_string(),
            class: "network".to_string(),
            description: "Intel Wi-Fi 6 AX200".to_string(),
            vendor_id: "8086".to_string(),
            device_id: "2723".to_string(),
            subsystem_id: None,
            kernel_driver: None,
            vram_mb: None,
        };
        let resolution = database().resolve(&device);
        assert_eq!(resolution.driver, "in-tree");
        assert_eq!(resolution.vendor.as_deref(), Some("Intel"));
    }

    #[test]
    fn a_table_from_a_later_format_is_refused_rather_than_misread() {
        // Same field names can mean different things in a later format, and
        // reading one anyway is how a machine gets the wrong driver.
        let mut directory = std::env::temp_dir();
        directory.push("xos-hw-format-test.json");
        std::fs::write(
            &directory,
            r#"{"format_version":99,"nvidia_branches":{"ranges":[],"branch_packages":{}}}"#,
        )
        .expect("write");
        let outcome = Database::load_from(&directory);
        assert!(outcome.is_err(), "a future format must not be read");
        // Specifically this reason, not a missing-field error: a later format
        // will usually fail to parse too, and the parse error would bury the
        // one fact that explains it.
        assert!(
            outcome.unwrap_err().contains("format 99"),
            "the reason given must be the version, not whatever failed to parse"
        );
        let _ = std::fs::remove_file(&directory);
    }

    #[test]
    fn the_shipped_table_is_the_format_the_loader_expects() {
        let database = database();
        assert!(!database.updated.is_empty());
        assert!(!database.entries.is_empty());
    }
}
