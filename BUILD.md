# XOS — Build Status

| # | ID | Task | Status | Gate |
|---|---|---|---|---|
| 1 | P0 | Repository scaffold | blocked | |
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

- P0 — blocked on toolchain, not on code. The scaffold is complete: workspace,
  three crates, nine `xosd` subsystem modules, `.gitignore`, README hardware
  table, MIT LICENSE. The check could not run because no Rust toolchain exists
  on this machine — `cargo` and `rustc` are absent from Windows (PATH,
  `~/.cargo`, Program Files, Chocolatey) and from WSL Ubuntu-24.04. Retrying
  cannot fix a missing compiler. `cargo build`, `cargo run --bin xos` and
  `cargo test` are unrun, not passed. Installing one is an operator decision:
  Windows needs the MSVC build tools, WSL needs only rustup (`cc` is already
  there), and WSL is the closer match to the Arch target.
- P0 decisions later tasks depend on: edition 2021 with `resolver = "2"`; crate
  metadata inherits from `[workspace.package]`; the `xos` binary name comes from
  an explicit `[[bin]]` in `xos-cli`, so `cargo run --bin xos` resolves without
  `-p`; subsystem modules are `src/<name>/mod.rs` directories, so files can be
  added without moving anything.
- LICENSE holder is "XOS contributors", a placeholder. Set a real name if wanted.
