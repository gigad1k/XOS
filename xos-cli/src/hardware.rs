//! `xos hardware` — what is in this machine, and what it needs.
//!
//! The rendering is a pure function over the daemon's answer so it can be tested
//! against a GTX 1080 without owning one. The command itself only fetches and
//! prints; every fact on the page comes from the daemon.

use serde_json::{json, Value};
use std::fmt::Write as _;

use crate::socket::Connection;

pub fn run(
    connection: &mut Connection,
    as_json: bool,
    device: Option<&str>,
    submit: bool,
) -> Result<String, String> {
    if submit {
        return self::submit(connection, as_json);
    }

    // Asking about a card that is not here is a question about the database,
    // not about this machine, so the inventory is left out of it entirely.
    if let Some(device) = device {
        let resolution = connection.call("hardware.resolve", json!({ "device": device }))?;
        if as_json {
            return serde_json::to_string_pretty(&json!({ "resolve": resolution }))
                .map_err(|e| format!("cannot render the report: {}", e));
        }
        return Ok(render_device(device, &resolution));
    }

    let inventory = connection.call("hardware.inventory", json!({}))?;
    let resolution = connection.call("hardware.resolve", json!({}))?;

    if as_json {
        // One document, so a script does not have to make several calls and
        // hope they describe the same machine. The submission report is in here
        // too, so the install script never has to assemble its own: two
        // descriptions of what would be sent is one too many.
        let report = connection
            .call("hardware.report", json!({}))
            .ok()
            .and_then(|answer| answer.get("report").cloned())
            .unwrap_or(Value::Null);
        let combined = json!({
            "inventory": inventory.get("inventory").cloned().unwrap_or(Value::Null),
            "profile": inventory.get("profile").cloned().unwrap_or(Value::Null),
            "summary": inventory.get("summary").cloned().unwrap_or(Value::Null),
            "resolve": resolution,
            "report": report,
        });
        return serde_json::to_string_pretty(&combined)
            .map_err(|e| format!("cannot render the report: {}", e));
    }

    Ok(render(&inventory, &resolution))
}

/// Everything `xos hardware` prints, from the two daemon answers.
pub fn render(inventory: &Value, resolution: &Value) -> String {
    let mut out = String::new();
    let machine = inventory.get("inventory").unwrap_or(&Value::Null);

    let text = |value: &Value, key: &str| -> String {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string()
    };
    let number = |value: &Value, key: &str| -> u64 {
        value.get(key).and_then(Value::as_u64).unwrap_or(0)
    };

    // CPU.
    if let Some(cpu) = machine.get("cpu") {
        let _ = writeln!(out, "cpu         {}", text(cpu, "model"));
        let level = number(cpu, "microarchitecture_level");
        let baseline = cpu
            .get("baseline_ok")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let _ = writeln!(
            out,
            "            {} cores, {} threads · x86-64-v{}{}",
            number(cpu, "cores"),
            number(cpu, "threads"),
            level,
            if baseline {
                ""
            } else {
                "  — below baseline x86-64, which XOS needs"
            }
        );
    }

    // Memory.
    if let Some(memory) = machine.get("memory") {
        let _ = writeln!(
            out,
            "memory      {} MB total, {} MB available",
            number(memory, "total_mb"),
            number(memory, "available_mb")
        );
    }

    // Firmware. The installer partitions on this answer, so it is stated
    // plainly rather than buried.
    if let Some(firmware) = machine.get("firmware") {
        let secure = match firmware.get("secure_boot").and_then(Value::as_bool) {
            Some(true) => " · secure boot on",
            Some(false) => " · secure boot off",
            None => "",
        };
        let _ = writeln!(
            out,
            "firmware    {}{}",
            text(firmware, "mode").to_uppercase(),
            secure
        );
    }

    let _ = writeln!(out);

    // Display devices, with the resolution beside each one, because the two
    // together are the answer someone actually wants.
    let display_resolutions = resolution
        .get("display")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let empty = Vec::new();
    let gpus = machine
        .get("gpus")
        .and_then(Value::as_array)
        .unwrap_or(&empty);

    if gpus.is_empty() {
        let _ = writeln!(out, "display     no display device found");
    }
    for (index, gpu) in gpus.iter().enumerate() {
        let _ = writeln!(out, "display     {}", text(gpu, "description"));
        let mut line = format!(
            "            {}:{}",
            text(gpu, "vendor_id"),
            text(gpu, "device_id")
        );
        if let Some(vram) = gpu.get("vram_mb").and_then(Value::as_u64) {
            let _ = write!(line, " · {} MB VRAM", vram);
        }
        if let Some(driver) = gpu.get("kernel_driver").and_then(Value::as_str) {
            let _ = write!(line, " · driver in use: {}", driver);
        }
        let _ = writeln!(out, "{}", line);

        if let Some(found) = display_resolutions.get(index) {
            if let Some(architecture) = found.get("architecture").and_then(Value::as_str) {
                match found.get("branch").and_then(Value::as_str) {
                    Some(branch) => {
                        let _ = writeln!(out, "            {} · branch {}", architecture, branch);
                    }
                    // Only NVIDIA has branches. A dash here would read as a
                    // missing fact rather than an absent concept.
                    None => {
                        let _ = writeln!(out, "            {}", architecture);
                    }
                }
            }
            if text(found, "driver") == "unknown" {
                // Calling "unknown" a recommendation invites someone to install
                // it. It is the absence of one.
                let _ = writeln!(out, "            XOS does not know this device");
            } else {
                let _ = writeln!(
                    out,
                    "            recommended: {} ({})",
                    text(found, "driver"),
                    text(found, "confidence")
                );
            }
            let fallback = chain(found);
            if !fallback.is_empty() {
                let _ = writeln!(out, "            if that fails: {}", fallback);
            }
            if let Some(why) = found.get("why").and_then(Value::as_str) {
                let _ = writeln!(out, "            {}", why);
            }
        }
    }

    // Network.
    let network_resolutions = resolution
        .get("network")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (index, nic) in machine
        .get("network")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
        .iter()
        .enumerate()
    {
        let _ = writeln!(out);
        let _ = writeln!(out, "network     {}", text(nic, "description"));
        if let Some(found) = network_resolutions.get(index) {
            let _ = writeln!(
                out,
                "            recommended: {} ({})",
                text(found, "driver"),
                text(found, "confidence")
            );
            let fallback = chain(found);
            if !fallback.is_empty() {
                let _ = writeln!(out, "            if that fails: {}", fallback);
            }
        }
    }

    // Storage.
    let disks = machine
        .get("storage")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    if !disks.is_empty() {
        let _ = writeln!(out);
        for disk in disks {
            let size = disk.get("size_gb").and_then(Value::as_f64).unwrap_or(0.0);
            let _ = writeln!(
                out,
                "storage     {} · {} · {}",
                text(disk, "name"),
                // A 400 MB disk printed as "0 GB" reads as a broken reading.
                if size < 1.0 {
                    format!("{:.0} MB", size * 1000.0)
                } else {
                    format!("{:.0} GB", size)
                },
                if disk
                    .get("rotational")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    "spinning"
                } else {
                    "solid state"
                }
            );
        }
    }

    // Displays actually plugged in.
    let connected: Vec<String> = machine
        .get("displays")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
        .iter()
        .filter(|display| {
            display
                .get("connected")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .map(|display| text(display, "output"))
        .collect();
    if !connected.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "outputs     {}", connected.join(", "));
    }

    // The one line most people came for.
    let _ = writeln!(out);
    let profile = inventory
        .get("profile")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let _ = writeln!(
        out,
        "profile     {}  — {}",
        profile,
        match profile {
            "local" => "this machine can run a model itself",
            "cpu" => "no usable GPU; a small model will run slowly on the CPU",
            "api-only" => "XOS will work through an API",
            _ => "could not tell",
        }
    );

    out
}

/// Send this machine to the community database.
///
/// The whole contents are printed first. Sending anything off someone's machine
/// without showing them what it is, in full, is not something XOS does — and a
/// summary is not the same as the thing itself.
fn submit(connection: &mut Connection, as_json: bool) -> Result<String, String> {
    let answer = connection.call("hardware.report", json!({}))?;
    let report = answer.get("report").cloned().unwrap_or(Value::Null);
    let body = serde_json::to_string_pretty(&report)
        .map_err(|e| format!("cannot render the report: {}", e))?;

    if as_json {
        return Ok(body);
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "This is exactly what would be sent to {}, and nothing else:",
        answer
            .get("endpoint")
            .and_then(Value::as_str)
            .unwrap_or("the community database")
    );
    let _ = writeln!(out);
    for line in body.lines() {
        let _ = writeln!(out, "  {}", line);
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "It describes the machine. It carries no hostname, no user name, no"
    );
    let _ = writeln!(out, "serial number and no network address.");
    let _ = writeln!(out);

    print!("{}", out);
    print!("Send it? [y/N] ");
    let _ = std::io::Write::flush(&mut std::io::stdout());

    let mut answer = String::new();
    if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_err() {
        return Ok("Not sent.".to_string());
    }
    if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
        return Ok("Not sent.".to_string());
    }

    let outcome = connection.call("hardware.submit", json!({ "confirm": true }))?;
    if outcome.get("sent").and_then(Value::as_bool) == Some(true) {
        Ok("Sent. Thank you — the next person with this card will have an easier time.".to_string())
    } else {
        // Someone who agreed to send something is owed the truth about whether
        // it went.
        Ok(format!(
            "Not sent: {}",
            outcome
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("the database could not be reached")
        ))
    }
}

/// One named card, with no machine around it.
fn render_device(name: &str, resolution: &Value) -> String {
    let mut out = String::new();
    let found = resolution
        .get("display")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .or_else(|| {
            resolution
                .get("network")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
        });

    let Some(found) = found else {
        let _ = writeln!(out, "device      {}", name);
        let _ = writeln!(out, "            nothing to say about it");
        return out;
    };

    let _ = writeln!(
        out,
        "device      {}{}",
        name,
        found
            .get("vendor")
            .and_then(Value::as_str)
            .map(|vendor| format!("  ({})", vendor))
            .unwrap_or_default()
    );
    if let Some(architecture) = found.get("architecture").and_then(Value::as_str) {
        match found.get("branch").and_then(Value::as_str) {
            Some(branch) => {
                let _ = writeln!(out, "            {} · branch {}", architecture, branch);
            }
            None => {
                let _ = writeln!(out, "            {}", architecture);
            }
        }
    }
    let _ = writeln!(
        out,
        "recommended {} ({})",
        found.get("driver").and_then(Value::as_str).unwrap_or("unknown"),
        found
            .get("confidence")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    );
    let parameters = found
        .get("kernel_parameters")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    if !parameters.is_empty() {
        let _ = writeln!(out, "kernel      {}", parameters);
    }
    let fallback = chain(found);
    if !fallback.is_empty() {
        let _ = writeln!(out, "if it fails {}", fallback);
    }
    if let Some(why) = found.get("why").and_then(Value::as_str) {
        let _ = writeln!(out, "            {}", why);
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "            This card is not in this machine. Nothing was installed,"
    );
    let _ = writeln!(out, "            loaded or changed to answer.");
    out
}

fn chain(resolution: &Value) -> String {
    resolution
        .get("fallback")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" → ")
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference machine from the build prompt, as the daemon would report
    /// it. Rendering is a pure function, so the check does not need the card.
    fn gtx_1080() -> (Value, Value) {
        let inventory = json!({
            "profile": "local",
            "summary": "GeForce GTX 1080",
            "inventory": {
                "cpu": {
                    "vendor": "GenuineIntel",
                    "model": "Intel(R) Core(TM) i7-4790K CPU @ 4.00GHz",
                    "cores": 4,
                    "threads": 8,
                    "microarchitecture_level": 3,
                    "baseline_ok": true,
                    "notable_flags": ["avx2"]
                },
                "memory": { "total_mb": 16000, "available_mb": 11000 },
                "gpus": [{
                    "bus": "pci", "slot": "01:00.0", "class": "display",
                    "description": "NVIDIA Corporation GP104 [GeForce GTX 1080] [10de:1b80]",
                    "vendor_id": "10de", "device_id": "1b80",
                    "kernel_driver": "nvidia", "vram_mb": 8192
                }],
                "network": [{
                    "bus": "pci", "slot": "03:00.0", "class": "network",
                    "description": "Realtek RTL8111 [10ec:8168]",
                    "vendor_id": "10ec", "device_id": "8168"
                }],
                "audio": [], "bluetooth": [],
                "storage": [{ "name": "sda", "size_gb": 512.1, "rotational": false }],
                "firmware": { "mode": "uefi", "secure_boot": false },
                "displays": [{ "output": "card0-DP-1", "connected": true, "driven_by": "card0" }]
            }
        });
        let resolution = json!({
            "display": [{
                "device": "GeForce GTX 1080",
                "vendor_id": "10de", "device_id": "1b80", "class": "display",
                "architecture": "Pascal", "driver": "nvidia-580xx-dkms", "branch": "580",
                "firmware": ["linux-firmware"],
                "kernel_parameters": ["nvidia_drm.modeset=1"],
                "fallback": ["nouveau", "vesa"],
                "confidence": "confirmed",
                "why": "GTX 1080 at 0x1B80 is the reference machine"
            }],
            "network": [{
                "device": "RTL8111", "vendor_id": "10ec", "device_id": "8168",
                "class": "network", "driver": "in-tree",
                "firmware": ["linux-firmware"], "kernel_parameters": [],
                "fallback": ["r8169", "r8168-dkms"], "confidence": "known"
            }],
            "profile": "local"
        });
        (inventory, resolution)
    }

    #[test]
    fn a_gtx_1080_reads_as_pascal_needing_the_580_branch() {
        // This is HW-1's check, run against the renderer.
        let (inventory, resolution) = gtx_1080();
        let output = render(&inventory, &resolution);
        assert!(output.contains("GeForce GTX 1080"), "{}", output);
        assert!(output.contains("Pascal"), "{}", output);
        assert!(output.contains("branch 580"), "{}", output);
        assert!(output.contains("nvidia-580xx-dkms"), "{}", output);
    }

    #[test]
    fn a_named_card_reports_its_branch_and_says_it_is_not_here() {
        let resolution = json!({
            "display": [{
                "device": "10de:1b80 (not in this machine)",
                "vendor": "NVIDIA", "vendor_id": "10de", "device_id": "1b80",
                "class": "display", "architecture": "Pascal",
                "driver": "nvidia-580xx-dkms", "branch": "580",
                "firmware": ["linux-firmware"],
                "kernel_parameters": ["nvidia_drm.modeset=1"],
                "fallback": ["nouveau", "vesa"],
                "confidence": "confirmed",
                "why": "GTX 1080 at 0x1B80 is the reference machine"
            }],
            "network": [],
            "hypothetical": true
        });
        let output = render_device("10de:1b80", &resolution);
        assert!(output.contains("Pascal"), "{}", output);
        assert!(output.contains("branch 580"), "{}", output);
        assert!(output.contains("nvidia-580xx-dkms"), "{}", output);
        assert!(
            output.contains("not in this machine"),
            "an answer about a card someone does not own must say so:\n{}",
            output
        );
    }

    #[test]
    fn the_firmware_mode_is_stated_plainly() {
        let (inventory, resolution) = gtx_1080();
        let output = render(&inventory, &resolution);
        assert!(output.contains("firmware    UEFI"), "{}", output);
        assert!(output.contains("secure boot off"), "{}", output);
    }

    #[test]
    fn the_fallback_chain_is_shown_in_order() {
        let (inventory, resolution) = gtx_1080();
        let output = render(&inventory, &resolution);
        assert!(
            output.contains("nouveau → vesa"),
            "someone whose screen went black needs the whole chain, in order:\n{}",
            output
        );
    }

    #[test]
    fn a_card_with_no_branch_does_not_print_an_empty_one() {
        // Only NVIDIA has driver branches. A dash where a fact should be reads
        // as a missing fact rather than an absent concept.
        let resolution = json!({
            "display": [{
                "vendor": "AMD", "vendor_id": "1002", "device_id": "6798",
                "class": "display", "architecture": "GCN 1.0 (Tahiti)",
                "driver": "mesa", "firmware": ["linux-firmware"],
                "kernel_parameters": ["amdgpu.si_support=1"],
                "fallback": ["amdgpu", "radeon", "vesa"], "confidence": "known"
            }],
            "network": []
        });
        let output = render_device("1002:6798", &resolution);
        assert!(output.contains("GCN 1.0 (Tahiti)"), "{}", output);
        assert!(!output.contains("branch"), "{}", output);
    }

    #[test]
    fn a_small_disk_is_not_rounded_to_nothing() {
        let (mut inventory, resolution) = gtx_1080();
        inventory["inventory"]["storage"] =
            json!([{ "name": "sda", "size_gb": 0.407, "rotational": false }]);
        let output = render(&inventory, &resolution);
        assert!(output.contains("407 MB"), "0 GB reads as a broken reading:\n{}", output);
    }

    #[test]
    fn a_machine_whose_firmware_cannot_be_seen_says_so() {
        // Not the same fact as legacy BIOS, and the installer must not treat it
        // as one.
        let (mut inventory, resolution) = gtx_1080();
        inventory["inventory"]["firmware"] = json!({ "mode": "unknown", "secure_boot": null });
        let output = render(&inventory, &resolution);
        assert!(output.contains("firmware    UNKNOWN"), "{}", output);
    }

    #[test]
    fn a_legacy_bios_machine_says_bios() {
        let (mut inventory, resolution) = gtx_1080();
        inventory["inventory"]["firmware"] = json!({ "mode": "bios", "secure_boot": null });
        let output = render(&inventory, &resolution);
        assert!(output.contains("firmware    BIOS"), "{}", output);
        assert!(!output.contains("secure boot"), "{}", output);
    }

    #[test]
    fn a_machine_below_baseline_is_told_so() {
        let (mut inventory, resolution) = gtx_1080();
        inventory["inventory"]["cpu"]["baseline_ok"] = json!(false);
        inventory["inventory"]["cpu"]["microarchitecture_level"] = json!(0);
        let output = render(&inventory, &resolution);
        assert!(
            output.contains("below baseline x86-64"),
            "a machine XOS cannot run on must be told before the install, not after:\n{}",
            output
        );
    }

    #[test]
    fn a_machine_with_no_gpu_still_renders() {
        let (mut inventory, _) = gtx_1080();
        inventory["inventory"]["gpus"] = json!([]);
        inventory["profile"] = json!("api-only");
        let output = render(&inventory, &json!({ "display": [], "network": [] }));
        assert!(output.contains("no display device found"), "{}", output);
        assert!(output.contains("XOS will work through an API"), "{}", output);
    }

    #[test]
    fn the_profile_carries_its_own_explanation() {
        let (inventory, resolution) = gtx_1080();
        let output = render(&inventory, &resolution);
        assert!(
            output.contains("profile     local  — this machine can run a model itself"),
            "{}",
            output
        );
    }

    #[test]
    fn an_unknown_card_is_reported_as_unknown_rather_than_guessed_at() {
        let (mut inventory, mut resolution) = gtx_1080();
        inventory["inventory"]["gpus"][0]["vendor_id"] = json!("dead");
        resolution["display"] = json!([{
            "device": "something new", "vendor_id": "dead", "device_id": "ffff",
            "class": "display", "driver": "unknown",
            "firmware": [], "kernel_parameters": [],
            "fallback": ["vesa"], "confidence": "unknown",
            "why": "not in the database; add a row if you get this working"
        }]);
        let output = render(&inventory, &resolution);
        assert!(
            output.contains("XOS does not know this device"),
            "calling `unknown` a recommendation invites someone to install it:
{}",
            output
        );
        assert!(!output.contains("recommended: unknown"), "{}", output);
        assert!(output.contains("add a row if you get this working"), "{}", output);
    }
}
