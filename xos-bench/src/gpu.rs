//! Peak VRAM sampling through nvidia-smi.
//!
//! Optional by design. On a machine with no NVIDIA driver the watch simply
//! reports nothing and the run carries on, because the harness has to work on
//! the same hardware range XOS targets.

use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Read used VRAM in megabytes, summed across devices.
pub fn sample_mb() -> Option<u64> {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=memory.used",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut total = 0u64;
    let mut seen = false;
    for line in text.lines() {
        if let Ok(value) = line.trim().parse::<u64>() {
            total += value;
            seen = true;
        }
    }
    if seen {
        Some(total)
    } else {
        None
    }
}

/// Samples VRAM on a background thread and keeps the highest reading.
pub struct VramWatch {
    peak: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl VramWatch {
    /// Returns None when nvidia-smi is absent, so callers can skip the metric.
    pub fn start() -> Option<Self> {
        let first = sample_mb()?;
        let peak = Arc::new(AtomicU64::new(first));
        let stop = Arc::new(AtomicBool::new(false));

        let thread_peak = Arc::clone(&peak);
        let thread_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                if let Some(used) = sample_mb() {
                    thread_peak.fetch_max(used, Ordering::Relaxed);
                }
                thread::sleep(Duration::from_millis(250));
            }
        });

        Some(Self {
            peak,
            stop,
            handle: Some(handle),
        })
    }

    /// Stop sampling and report the peak in megabytes.
    pub fn finish(mut self) -> u64 {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        self.peak.load(Ordering::Relaxed)
    }
}
