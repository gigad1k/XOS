# XOS — Build Status

| # | ID | Task | Status | Gate |
|---|---|---|---|---|
| 1 | P0 | Repository scaffold | done | |
| 2 | P1 | XOS Bench — tool-call harness | todo | *GATE* |
| 3 | P2 | xosd skeleton, provider trait, halt primitive | todo | |
| 4 | P3 | xos chat TUI | todo | |
| 5 | P4 | XOS Vault and cloud providers | todo | |
| 6 | P5 | Router and escalation log | todo | *GATE* |
| 7 | P6 | XOS Memory, export and import | todo | |
| 8 | P7 | Policy engine and egress protection | todo | *GATE* |
| 9 | P7b | Action journal and undo | todo | *GATE* |
| 10 | P8 | Supervisor | todo | |
| 11 | P9 | XOS Goals, the task graph | todo | |
| 12 | P10 | XOS Pulse and power management | todo | |
| 13 | P11 | Status bar and desktop theme | todo | |
| 14 | P12 | Mission Control | todo | |
| 15 | HW-1 | Hardware detection | todo | |
| 16 | HW-2 | Driver resolution and installation | todo | *GATE* |
| 17 | P13 | Installer | todo | *GATE* |
| 18 | P14 | First-run experience and first goal | todo | |

## Status values
todo · in-progress · done · blocked

## Gates
A task marked GATE must stop after completion and wait for human review. Never
begin the task after a gate in the same invocation. Gates exist where a result
changes architecture, where code is security-critical, or where the work touches
real disks.

## Notes
(append findings, deviations and blockers here as the build proceeds)

- P0 — done. Check passed: `cargo build` compiles all three crates with zero
  warnings, `cargo run --bin xos` exits 0, and `cargo test` is green.
- Build environment, which every later task depends on: there is no Rust
  toolchain on Windows and none was installed there. The workspace builds under
  WSL Ubuntu-24.04 with rustup stable (rustc 1.98.1), run from
  /mnt/c/Users/shahr/Claude/Projects/XOS. Build on that side, not the Windows
  side — Windows-native would need the MSVC build tools and could not run the
  Linux work in HW-1, HW-2 and P13 regardless.
- Cargo.lock is committed, as it should be for a workspace that ships binaries.
- P0 decisions later tasks depend on: edition 2021 with `resolver = "2"`; crate
  metadata inherits from `[workspace.package]`; the `xos` binary name comes from
  an explicit `[[bin]]` in `xos-cli`, so `cargo run --bin xos` resolves without
  `-p`; subsystem modules are `src/<name>/mod.rs` directories, so files can be
  added without moving anything.
- LICENSE holder is "XOS contributors", a placeholder. Set a real name if wanted.
