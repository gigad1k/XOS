//! System state.
//!
//! The daemon's live view of the machine — hardware, services, resources and
//! the halt flag that stops every autonomous action.
//!
//! # The halt primitive
//!
//! Halt is XOS's kill switch, and it is built before there is anything to halt
//! so that no subsystem is ever written without it. Three properties matter, and
//! each one shapes the implementation:
//!
//! *It always works.* The flag is an atomic, never a mutex. A completion that
//! holds a lock cannot delay a halt, because halting takes no lock the
//! completion could be holding. Reading the flag is an atomic load, so checking
//! it costs nothing and every subsystem can afford to check it often.
//!
//! *It takes effect immediately.* Halting sets the flag, then wakes every
//! in-flight task through a broadcast channel, and only then writes to disk. A
//! slow disk cannot delay the stop.
//!
//! *It survives a restart.* The flag is persisted, so a halted machine comes
//! back halted. Resuming is something a person does on purpose, never something
//! that happens because a daemon restarted.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::broadcast;

/// Returned by [`Halt::guard`] when the system is halted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Halted;

impl std::fmt::Display for Halted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "halted")
    }
}

impl std::error::Error for Halted {}

pub struct Halt {
    halted: AtomicBool,
    cancel: broadcast::Sender<()>,
    state_path: PathBuf,
}

impl Halt {
    /// Build from persisted state. A machine that was halted stays halted.
    pub fn load(state_path: impl Into<PathBuf>) -> Self {
        let state_path = state_path.into();
        let halted = state_path.exists();
        let (cancel, _) = broadcast::channel(64);
        Self {
            halted: AtomicBool::new(halted),
            cancel,
            state_path,
        }
    }

    /// Lock-free, so every subsystem can afford to check before acting.
    pub fn is_halted(&self) -> bool {
        self.halted.load(Ordering::SeqCst)
    }

    /// The check a subsystem makes before doing anything autonomous.
    pub fn guard(&self) -> Result<(), Halted> {
        if self.is_halted() {
            Err(Halted)
        } else {
            Ok(())
        }
    }

    /// Wake up when a halt happens. In-flight work selects on this.
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.cancel.subscribe()
    }

    /// Stop everything. Flag first, waiters second, disk last, so neither a
    /// busy daemon nor a slow disk can delay the stop.
    pub fn halt(&self) -> io::Result<()> {
        self.halted.store(true, Ordering::SeqCst);
        // Fails only when nothing is listening, which is not an error here.
        let _ = self.cancel.send(());
        self.persist(true)
    }

    /// Clear the halt. Deliberate, and only ever done by a person.
    pub fn resume(&self) -> io::Result<()> {
        self.halted.store(false, Ordering::SeqCst);
        self.persist(false)
    }

    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    fn persist(&self, halted: bool) -> io::Result<()> {
        if halted {
            if let Some(parent) = self.state_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&self.state_path, b"halted\n")
        } else {
            match fs::remove_file(&self.state_path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_state() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("halted");
        (dir, path)
    }

    #[test]
    fn starts_running() {
        let (_dir, path) = temp_state();
        let halt = Halt::load(&path);
        assert!(!halt.is_halted());
        assert!(halt.guard().is_ok());
    }

    #[test]
    fn halting_blocks_the_guard() {
        let (_dir, path) = temp_state();
        let halt = Halt::load(&path);
        halt.halt().expect("halt persists");
        assert!(halt.is_halted());
        assert_eq!(halt.guard(), Err(Halted));
    }

    #[test]
    fn a_halt_survives_a_restart() {
        let (_dir, path) = temp_state();
        Halt::load(&path).halt().expect("halt persists");

        // A fresh daemon reading the same state must come back halted.
        let restarted = Halt::load(&path);
        assert!(
            restarted.is_halted(),
            "a restart must not silently resume autonomous work"
        );
    }

    #[test]
    fn resuming_clears_the_flag_and_the_file() {
        let (_dir, path) = temp_state();
        let halt = Halt::load(&path);
        halt.halt().expect("halt persists");
        halt.resume().expect("resume clears");
        assert!(!halt.is_halted());
        assert!(!path.exists());
        assert!(!Halt::load(&path).is_halted());
    }

    #[test]
    fn resuming_twice_is_not_an_error() {
        let (_dir, path) = temp_state();
        let halt = Halt::load(&path);
        halt.resume().expect("first resume");
        halt.resume().expect("second resume");
    }

    #[tokio::test]
    async fn in_flight_work_is_woken() {
        let (_dir, path) = temp_state();
        let halt = Halt::load(&path);
        let mut waiter = halt.subscribe();
        halt.halt().expect("halt persists");
        // The signal is already queued, so this returns without waiting.
        waiter.recv().await.expect("in-flight work is notified");
    }
}
