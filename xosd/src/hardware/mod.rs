//! Hardware detection.
//!
//! XOS targets old and varied PCs, so what is in the box decides three things:
//! which drivers the installer picks, whether the machine runs a local model at
//! all, and what the community database learns.
//!
//! # Read-only, absolutely
//!
//! This module inventories and resolves. It never installs, loads, modifies or
//! probes anything that could change state. Everything here reads `/proc`,
//! `/sys`, or the output of a tool that itself only reads. Installation belongs
//! to HW-2, and keeping that boundary means detection can be run freely, by
//! anyone, at any time, including on a machine that is already unhappy.

pub mod db;

/// Where a submission goes. One place, so the CLI, the daemon and the install
/// script cannot disagree about it.
pub const SUBMIT_URL: &str = "https://hardware.xos.community/submit";

use serde::Serialize;

pub use db::Database;

#[derive(Debug, Clone, Serialize)]
pub struct Inventory {
    pub cpu: Cpu,
    pub memory: MemoryInfo,
    pub gpus: Vec<Device>,
    pub network: Vec<Device>,
    pub audio: Vec<Device>,
    pub bluetooth: Vec<Device>,
    pub storage: Vec<Storage>,
    pub firmware: Firmware,
    pub displays: Vec<Display>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Cpu {
    pub vendor: String,
    pub model: String,
    pub cores: usize,
    pub threads: usize,
    /// The x86-64 level the flags actually support: 1, 2, 3 or 4.
    pub microarchitecture_level: u8,
    /// False when the CPU cannot even do baseline x86-64, which XOS requires.
    pub baseline_ok: bool,
    pub notable_flags: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryInfo {
    pub total_mb: u64,
    pub available_mb: u64,
}

/// A PCI or USB device, as the kernel sees it.
#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub bus: String,
    pub slot: String,
    pub class: String,
    pub description: String,
    pub vendor_id: String,
    pub device_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subsystem_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kernel_driver: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vram_mb: Option<u64>,
}

impl Device {
    /// A device named rather than found, written `vendor:device` in hex.
    ///
    /// Nothing about it is read from this machine, so it carries no driver in
    /// use and no VRAM. It exists to answer "what would XOS do with this card?"
    /// before anyone has to own one.
    pub fn named(text: &str, class: &str) -> Option<Self> {
        let (vendor, device) = text.trim().split_once(':')?;
        let vendor = vendor.trim().trim_start_matches("0x").to_lowercase();
        let device = device.trim().trim_start_matches("0x").to_lowercase();
        if vendor.len() != 4
            || device.len() != 4
            || !vendor.chars().all(|c| c.is_ascii_hexdigit())
            || !device.chars().all(|c| c.is_ascii_hexdigit())
        {
            return None;
        }
        Some(Device {
            bus: "pci".to_string(),
            slot: String::new(),
            class: class.to_string(),
            description: format!("{}:{} (not in this machine)", vendor, device),
            vendor_id: vendor,
            device_id: device,
            subsystem_id: None,
            kernel_driver: None,
            vram_mb: None,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Storage {
    pub name: String,
    pub size_gb: f64,
    pub rotational: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smart: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Firmware {
    /// "uefi", "bios", or "unknown" when the firmware is not visible at all.
    ///
    /// The third state is not hedging. A container, a VM or WSL has no firmware
    /// to see, and answering "bios" there would tell the installer to lay down
    /// an MBR on a machine that boots UEFI. Not knowing is a different fact
    /// from knowing it is legacy, and the installer has to treat it that way.
    pub mode: String,
    pub secure_boot: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Display {
    pub output: String,
    pub connected: bool,
    /// Which card drives it, by its /sys card name.
    pub driven_by: String,
}

/// The anonymised report for the community database.
///
/// Built here, in the daemon, so that the CLI and the install script cannot
/// drift into sending different things. What is left out is the point: no
/// hostname, no user names, no MAC addresses, no serial numbers, no disk
/// contents. Hardware identifiers describe the machine; nothing here describes
/// the person sitting at it.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub format: &'static str,
    pub format_version: u32,
    pub cpu_model: String,
    pub cpu_level: u8,
    pub memory_mb: u64,
    pub firmware: String,
    pub profile: &'static str,
    pub display: Vec<ReportDevice>,
    pub network: Vec<ReportDevice>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReportDevice {
    pub vendor_id: String,
    pub device_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subsystem_id: Option<String>,
    pub class: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kernel_driver: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_driver: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
}

impl Inventory {
    /// What would be sent, and nothing else.
    pub fn report(&self, database: Option<&Database>) -> Report {
        let describe = |devices: &[Device]| -> Vec<ReportDevice> {
            devices
                .iter()
                .map(|device| {
                    let resolution = database.map(|db| db.resolve(device));
                    ReportDevice {
                        vendor_id: device.vendor_id.clone(),
                        device_id: device.device_id.clone(),
                        subsystem_id: device.subsystem_id.clone(),
                        class: device.class.clone(),
                        kernel_driver: device.kernel_driver.clone(),
                        resolved_driver: resolution.as_ref().map(|r| r.driver.clone()),
                        branch: resolution.as_ref().and_then(|r| r.branch.clone()),
                        confidence: resolution.as_ref().map(|r| r.confidence.clone()),
                    }
                })
                .collect()
        };

        Report {
            format: "xos-hardware-report",
            format_version: 1,
            cpu_model: self.cpu.model.clone(),
            cpu_level: self.cpu.microarchitecture_level,
            memory_mb: self.memory.total_mb,
            firmware: self.firmware.mode.clone(),
            profile: self.profile().label(),
            display: describe(&self.gpus),
            network: describe(&self.network),
        }
    }
}

/// Which tier of model this machine can realistically carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    /// A GPU with enough VRAM for a local model.
    Local,
    /// No usable GPU, but enough CPU and memory to run something small slowly.
    Cpu,
    /// Neither. XOS still works, through an API.
    ApiOnly,
}

impl Profile {
    pub fn label(&self) -> &'static str {
        match self {
            Profile::Local => "local",
            Profile::Cpu => "cpu",
            Profile::ApiOnly => "api-only",
        }
    }
}

/// VRAM below this cannot hold a useful model alongside a desktop.
///
/// Written against what a card reports rather than what it is sold as. A card
/// sold as 4GB reports a little under 4096 once its own firmware has taken a
/// share, so the round number would exclude every 4GB card there is.
const MIN_LOCAL_VRAM_MB: u64 = 3800;

/// Below this much system memory, CPU inference is not worth offering.
///
/// Same reasoning, and it matters more here. A machine sold as 8GB reports
/// something near 7800 MB, because firmware and an integrated GPU take their
/// cut before Linux sees any of it. At 8192 this said `api-only` for every 8GB
/// machine on earth, and the specification says in as many words that 8GB
/// machines are what XOS is for: it would have told the owner of its own target
/// machine that it could not run a local model.
const MIN_CPU_MEMORY_MB: u64 = 7600;

impl Inventory {
    pub fn read() -> Self {
        let pci = read_pci();
        Self {
            cpu: read_cpu(),
            memory: read_memory(),
            gpus: pci.iter().filter(|d| d.class == "display").cloned().collect(),
            network: pci.iter().filter(|d| d.class == "network").cloned().collect(),
            audio: pci.iter().filter(|d| d.class == "audio").cloned().collect(),
            bluetooth: read_bluetooth(),
            storage: read_storage(),
            firmware: read_firmware(),
            displays: read_displays(),
        }
    }

    /// What this machine can run.
    ///
    /// Deliberately conservative. Promising local inference on a machine that
    /// cannot deliver it produces a bad first hour, and XOS works perfectly well
    /// through an API while someone decides what to do about the GPU.
    pub fn profile(&self) -> Profile {
        let best_vram = self
            .gpus
            .iter()
            .filter_map(|gpu| gpu.vram_mb)
            .max()
            .unwrap_or(0);

        if best_vram >= MIN_LOCAL_VRAM_MB {
            Profile::Local
        } else if self.cpu.baseline_ok && self.memory.total_mb >= MIN_CPU_MEMORY_MB {
            Profile::Cpu
        } else {
            Profile::ApiOnly
        }
    }

    /// A sentence a person can read.
    pub fn summary(&self) -> String {
        let gpu = self
            .gpus
            .first()
            .map(|gpu| gpu.description.clone())
            .unwrap_or_else(|| "no display device".to_string());
        format!(
            "{} · {} cores, {} threads · {} MB memory · {} · firmware {}",
            gpu,
            self.cpu.cores,
            self.cpu.threads,
            self.memory.total_mb,
            self.profile().label(),
            self.firmware.mode
        )
    }
}

fn read_cpu() -> Cpu {
    let text = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let mut vendor = String::new();
    let mut model = String::new();
    let mut flags: Vec<String> = Vec::new();
    let mut threads = 0usize;
    let mut cores = 0usize;

    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "vendor_id" if vendor.is_empty() => vendor = value.to_string(),
            "model name" if model.is_empty() => model = value.to_string(),
            "processor" => threads += 1,
            "cpu cores" if cores == 0 => cores = value.parse().unwrap_or(0),
            "flags" if flags.is_empty() => {
                flags = value.split_whitespace().map(str::to_string).collect()
            }
            _ => {}
        }
    }
    if cores == 0 {
        cores = threads;
    }

    let has = |flag: &str| flags.iter().any(|f| f == flag);
    // Baseline x86-64. XOS targets this and nothing above it, because raising
    // the floor would exclude exactly the machines it exists for.
    let baseline_ok = ["cmov", "cx8", "fpu", "fxsr", "mmx", "syscall", "sse", "sse2"]
        .iter()
        .all(|flag| has(flag));
    let v2 = ["popcnt", "sse4_2", "ssse3", "cx16"].iter().all(|f| has(f));
    let v3 = v2 && ["avx", "avx2", "bmi1", "bmi2", "fma"].iter().all(|f| has(f));
    let v4 = v3 && has("avx512f");

    let level = if v4 {
        4
    } else if v3 {
        3
    } else if v2 {
        2
    } else if baseline_ok {
        1
    } else {
        0
    };

    let notable = ["avx", "avx2", "avx512f", "aes", "sse4_2", "popcnt", "f16c"]
        .iter()
        .filter(|flag| has(flag))
        .map(|flag| flag.to_string())
        .collect();

    Cpu {
        vendor: if vendor.is_empty() {
            "unknown".to_string()
        } else {
            vendor
        },
        model: if model.is_empty() {
            "unknown".to_string()
        } else {
            model
        },
        cores,
        threads,
        microarchitecture_level: level,
        baseline_ok,
        notable_flags: notable,
    }
}

fn read_memory() -> MemoryInfo {
    let text = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut total = 0u64;
    let mut available = 0u64;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        match (fields.next(), fields.next()) {
            (Some("MemTotal:"), Some(value)) => total = value.parse().unwrap_or(0),
            (Some("MemAvailable:"), Some(value)) => available = value.parse().unwrap_or(0),
            _ => {}
        }
    }
    MemoryInfo {
        total_mb: total / 1024,
        available_mb: available / 1024,
    }
}

/// Every PCI device, from sysfs.
///
/// The kernel is the source, not `lspci`. A minimal install image very often
/// has no pciutils, and that is exactly the machine that most needs its
/// graphics card identified. `lspci` is still used afterwards, if it happens to
/// be there, but only to put human-readable names on devices the kernel has
/// already told us about.
fn read_pci() -> Vec<Device> {
    let Ok(entries) = std::fs::read_dir("/sys/bus/pci/devices") else {
        return Vec::new();
    };
    let names = lspci_names();
    let mut devices = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let address = entry.file_name().to_string_lossy().to_string();
        // sysfs writes the domain; lspci and everyone else drop it when it is
        // zero, so the slot is shown the way people write it.
        let slot = address
            .strip_prefix("0000:")
            .unwrap_or(&address)
            .to_string();

        let Some(vendor_id) = sysfs_id(&path, "vendor") else {
            continue;
        };
        let Some(device_id) = sysfs_id(&path, "device") else {
            continue;
        };
        let class_code = sysfs_hex(&path, "class").unwrap_or(0);
        let class = pci_class(class_code);
        if class == "other" {
            continue;
        }

        // A subsystem of 0000:0000 is the kernel saying there is no subsystem,
        // not a subsystem that happens to be zero. Reporting it as a value puts
        // a meaningless row in the community database.
        let subsystem_id = match (
            sysfs_id(&path, "subsystem_vendor"),
            sysfs_id(&path, "subsystem_device"),
        ) {
            (Some(vendor), Some(device)) if vendor != "0000" || device != "0000" => {
                Some(format!("{}:{}", vendor, device))
            }
            _ => None,
        };

        // The driver is a symlink to the module that claimed the device, so an
        // empty answer means nothing has claimed it — which is itself the
        // interesting case.
        let kernel_driver = std::fs::read_link(path.join("driver"))
            .ok()
            .and_then(|target| {
                target
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
            });

        let description = names.get(&slot).cloned().unwrap_or_else(|| {
            format!("{} device {}:{}", class, vendor_id, device_id)
        });

        let vram_mb = if class == "display" {
            read_vram(&path, &vendor_id, &slot)
        } else {
            None
        };

        devices.push(Device {
            bus: "pci".to_string(),
            slot,
            class: class.to_string(),
            description,
            vendor_id,
            device_id,
            subsystem_id,
            kernel_driver,
            vram_mb,
        });
    }

    devices.sort_by(|a, b| a.slot.cmp(&b.slot));
    devices
}

/// The PCI base class, which is the first byte of the class code.
fn pci_class(code: u32) -> &'static str {
    match code >> 16 {
        0x03 => "display",
        0x02 => "network",
        0x04 => "audio",
        _ => "other",
    }
}

fn sysfs_hex(path: &std::path::Path, name: &str) -> Option<u32> {
    let text = std::fs::read_to_string(path.join(name)).ok()?;
    u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
}

/// A four-digit lowercase PCI ID, the way the database is keyed.
fn sysfs_id(path: &std::path::Path, name: &str) -> Option<String> {
    sysfs_hex(path, name).map(|value| format!("{:04x}", value))
}

/// Readable names, if pciutils happens to be installed. Names only: every fact
/// that matters has already come from the kernel.
fn lspci_names() -> std::collections::HashMap<String, String> {
    let mut names = std::collections::HashMap::new();
    let Ok(output) = std::process::Command::new("lspci").args(["-nn"]).output() else {
        return names;
    };
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some((slot, rest)) = line.split_once(' ') {
            if let Some((_class, description)) = rest.split_once(':') {
                names.insert(
                    slot.trim_start_matches("0000:").to_string(),
                    description.trim().to_string(),
                );
            }
        }
    }
    names
}

/// How much memory this card has.
///
/// Matched to the card by its PCI address in both branches. Asking either tool
/// for "the" GPU and taking the first answer would report the wrong number on
/// any machine with two cards, which is the machine most likely to care.
fn read_vram(path: &std::path::Path, vendor_id: &str, slot: &str) -> Option<u64> {
    // AMD and Intel put it in sysfs, on the device itself.
    if let Ok(text) = std::fs::read_to_string(path.join("mem_info_vram_total")) {
        if let Ok(bytes) = text.trim().parse::<u64>() {
            return Some(bytes / 1_048_576);
        }
    }

    if vendor_id == "10de" {
        let output = std::process::Command::new("nvidia-smi")
            .args([
                "--query-gpu=pci.bus_id,memory.total",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .ok()?;
        if output.status.success() {
            let wanted = slot.to_lowercase();
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let (bus_id, memory) = line.split_once(',')?;
                // nvidia-smi writes the full domain in upper case.
                let bus_id = bus_id.trim().to_lowercase();
                let bus_id = bus_id.strip_prefix("0000:").unwrap_or(&bus_id);
                if bus_id == wanted {
                    return memory.trim().parse().ok();
                }
            }
        }
    }
    None
}

fn read_bluetooth() -> Vec<Device> {
    let Ok(output) = std::process::Command::new("lsusb").output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.to_lowercase().contains("bluetooth"))
        .filter_map(|line| {
            let ids = line.split_whitespace().nth(5)?;
            let (vendor, device) = ids.split_once(':')?;
            Some(Device {
                bus: "usb".to_string(),
                slot: String::new(),
                class: "bluetooth".to_string(),
                description: line.to_string(),
                vendor_id: vendor.to_lowercase(),
                device_id: device.to_lowercase(),
                subsystem_id: None,
                kernel_driver: None,
                vram_mb: None,
            })
        })
        .collect()
}

fn read_storage() -> Vec<Storage> {
    let mut disks = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/block") else {
        return disks;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("loop") || name.starts_with("ram") || name.starts_with("zram") {
            continue;
        }
        let sectors: u64 = std::fs::read_to_string(entry.path().join("size"))
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0);
        if sectors == 0 {
            continue;
        }
        let rotational = std::fs::read_to_string(entry.path().join("queue/rotational"))
            .map(|text| text.trim() == "1")
            .unwrap_or(false);
        disks.push(Storage {
            name,
            // 512-byte sectors, which is what /sys reports regardless of the
            // physical block size.
            size_gb: (sectors as f64 * 512.0) / 1e9,
            rotational,
            smart: None,
        });
    }
    disks
}

fn read_firmware() -> Firmware {
    // The presence of the EFI runtime is the standard test, and the same one the
    // installer has to make before it decides how to partition.
    let uefi = std::path::Path::new("/sys/firmware/efi").exists();

    // If there is no EFI runtime, the question is whether this is a legacy boot
    // or a place with no firmware at all. DMI is the tell: real x86 firmware
    // publishes it, and a container or WSL does not.
    let firmware_is_visible = std::path::Path::new("/sys/class/dmi/id").exists()
        || std::path::Path::new("/sys/firmware/dmi").exists();

    let secure_boot = if uefi {
        std::fs::read_dir("/sys/firmware/efi/efivars")
            .ok()
            .and_then(|entries| {
                entries.flatten().find_map(|entry| {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !name.starts_with("SecureBoot-") {
                        return None;
                    }
                    let bytes = std::fs::read(entry.path()).ok()?;
                    // Four attribute bytes, then the value.
                    bytes.get(4).map(|value| *value == 1)
                })
            })
    } else {
        None
    };

    let mode = if uefi {
        "uefi"
    } else if firmware_is_visible {
        "bios"
    } else {
        "unknown"
    };

    Firmware {
        mode: mode.to_string(),
        secure_boot,
    }
}

fn read_displays() -> Vec<Display> {
    let mut displays = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return displays;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.contains('-') {
            continue;
        }
        let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
            continue;
        };
        let card = name.split('-').next().unwrap_or("card0").to_string();
        displays.push(Display {
            output: name,
            connected: status.trim() == "connected",
            driven_by: card,
        });
    }
    displays
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_the_machine_never_panics() {
        let inventory = Inventory::read();
        let _ = inventory.summary();
        let _ = inventory.profile();
    }

    #[test]
    fn the_report_describes_the_machine_and_never_the_person() {
        // The value of this report is entirely in the device identifiers. Every
        // field that could name the person at the keyboard is absent by
        // construction, and this test is what keeps it that way as fields are
        // added later.
        let report = capable().report(None);
        let text = serde_json::to_string(&report).expect("serialise");
        for forbidden in [
            "hostname", "serial", "uuid", "mac", "user", "home", "ssid", "address",
        ] {
            assert!(
                !text.to_lowercase().contains(forbidden),
                "`{}` appears in a report that is meant to describe only hardware: {}",
                forbidden,
                text
            );
        }
        assert_eq!(report.display.len(), 1, "the device itself is still there");
        assert_eq!(report.display[0].device_id, "1b80");
    }

    #[test]
    fn a_named_device_is_parsed_in_either_spelling() {
        for text in ["10de:1b80", "0x10DE:0x1B80", " 10de : 1b80 "] {
            let device = Device::named(text, "display").expect(text);
            assert_eq!(device.vendor_id, "10de");
            assert_eq!(device.device_id, "1b80");
            assert!(device.kernel_driver.is_none(), "nothing is read from this machine");
        }
    }

    #[test]
    fn a_malformed_device_name_is_refused_rather_than_half_read() {
        for text in ["10de", "10de:", "zzzz:1b80", "10de:1b8", ""] {
            assert!(Device::named(text, "display").is_none(), "{} was accepted", text);
        }
    }

    #[test]
    fn an_absent_subsystem_is_absent_rather_than_zero() {
        for device in read_pci() {
            assert_ne!(
                device.subsystem_id.as_deref(),
                Some("0000:0000"),
                "0000:0000 means there is no subsystem, and the community \
                 database would collect a meaningless row for it"
            );
        }
    }

    #[test]
    fn pci_base_classes_are_read_from_the_class_code() {
        assert_eq!(pci_class(0x030000), "display", "VGA controller");
        assert_eq!(pci_class(0x030200), "display", "3D controller");
        assert_eq!(pci_class(0x020000), "network", "ethernet");
        assert_eq!(pci_class(0x028000), "network", "other network");
        assert_eq!(pci_class(0x040300), "audio", "audio device");
        assert_eq!(pci_class(0x010601), "other", "a SATA controller is not one of ours");
    }

    #[test]
    fn every_pci_device_reported_carries_the_ids_the_database_is_keyed_on() {
        // Whatever this machine is, nothing half-read may be reported: a device
        // without both IDs cannot be resolved and must not appear as if it could.
        for device in read_pci() {
            assert_eq!(device.vendor_id.len(), 4, "{:?}", device);
            assert_eq!(device.device_id.len(), 4, "{:?}", device);
            assert!(device.vendor_id.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(device.device_id.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn detection_does_not_need_pciutils_installed() {
        // The machine that most needs its graphics card identified is a minimal
        // install image, which very often has no lspci. Names may be missing;
        // facts may not.
        let names = lspci_names();
        for device in read_pci() {
            if names.is_empty() {
                assert!(
                    !device.description.is_empty(),
                    "a device still has to be describable without lspci"
                );
            }
        }
    }

    #[test]
    fn a_machine_with_a_capable_gpu_runs_locally() {
        let inventory = Inventory {
            cpu: Cpu {
                vendor: "GenuineIntel".to_string(),
                model: "test".to_string(),
                cores: 4,
                threads: 8,
                microarchitecture_level: 1,
                baseline_ok: true,
                notable_flags: Vec::new(),
            },
            memory: MemoryInfo {
                total_mb: 32_000,
                available_mb: 16_000,
            },
            gpus: vec![Device {
                bus: "pci".to_string(),
                slot: "01:00.0".to_string(),
                class: "display".to_string(),
                description: "GeForce GTX 1080".to_string(),
                vendor_id: "10de".to_string(),
                device_id: "1b80".to_string(),
                subsystem_id: None,
                kernel_driver: Some("nvidia".to_string()),
                vram_mb: Some(8192),
            }],
            network: Vec::new(),
            audio: Vec::new(),
            bluetooth: Vec::new(),
            storage: Vec::new(),
            firmware: Firmware {
                mode: "uefi".to_string(),
                secure_boot: Some(false),
            },
            displays: Vec::new(),
        };
        assert_eq!(inventory.profile(), Profile::Local);
    }

    #[test]
    fn a_machine_with_no_usable_gpu_falls_back_to_cpu() {
        let mut inventory = capable();
        inventory.gpus[0].vram_mb = Some(512);
        assert_eq!(inventory.profile(), Profile::Cpu);
    }

    #[test]
    fn a_real_8gb_machine_is_not_told_it_is_too_small() {
        // The machine XOS exists for. A threshold of 8192 excluded it, because
        // no 8GB machine ever reports 8192.
        let mut inventory = capable();
        inventory.gpus.clear();
        inventory.memory.total_mb = 7800;
        assert_eq!(
            inventory.profile(),
            Profile::Cpu,
            "an 8GB machine was told it could not run a local model"
        );
    }

    #[test]
    fn a_real_4gb_card_counts_as_a_usable_gpu() {
        let mut inventory = capable();
        inventory.gpus[0].vram_mb = Some(4020);
        assert_eq!(inventory.profile(), Profile::Local);
    }

    #[test]
    fn a_small_old_machine_is_api_only() {
        let mut inventory = capable();
        inventory.gpus.clear();
        inventory.memory.total_mb = 4096;
        assert_eq!(
            inventory.profile(),
            Profile::ApiOnly,
            "promising local inference it cannot deliver makes a bad first hour"
        );
    }

    fn capable() -> Inventory {
        Inventory {
            cpu: Cpu {
                vendor: "AuthenticAMD".to_string(),
                model: "test".to_string(),
                cores: 8,
                threads: 16,
                microarchitecture_level: 3,
                baseline_ok: true,
                notable_flags: vec!["avx2".to_string()],
            },
            memory: MemoryInfo {
                total_mb: 32_000,
                available_mb: 20_000,
            },
            gpus: vec![Device {
                bus: "pci".to_string(),
                slot: "01:00.0".to_string(),
                class: "display".to_string(),
                description: "test gpu".to_string(),
                vendor_id: "10de".to_string(),
                device_id: "1b80".to_string(),
                subsystem_id: None,
                kernel_driver: None,
                vram_mb: Some(8192),
            }],
            network: Vec::new(),
            audio: Vec::new(),
            bluetooth: Vec::new(),
            storage: Vec::new(),
            firmware: Firmware {
                mode: "bios".to_string(),
                secure_boot: None,
            },
            displays: Vec::new(),
        }
    }

    #[test]
    fn firmware_reports_one_of_three_states_and_nothing_else() {
        let firmware = read_firmware();
        assert!(
            ["uefi", "bios", "unknown"].contains(&firmware.mode.as_str()),
            "the installer partitions on this answer, so it cannot be vague: {}",
            firmware.mode
        );
    }

    #[test]
    fn secure_boot_is_only_claimed_when_there_is_efi_to_ask() {
        let firmware = read_firmware();
        if firmware.mode != "uefi" {
            assert!(
                firmware.secure_boot.is_none(),
                "there is no secure boot to report without EFI"
            );
        }
    }
}
