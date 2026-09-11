# XOS — Build Status

| # | ID | Task | Status | Gate |
|---|---|---|---|---|
| 1 | P0 | Repository scaffold | done | |
| 2 | P1 | XOS Bench — tool-call harness | done | *GATE* |
| 3 | P2 | xosd skeleton, provider trait, halt primitive | done | |
| 4 | P3 | xos chat TUI | done | |
| 5 | P4 | XOS Vault and cloud providers | done | |
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

- P1 — done. Check passed: `cargo run --bin xos-bench -- --url http://localhost:11434/v1`
  prints the table with a tool-call success percentage and exits 0. The harness
  ships 50 cases, ten in each of the five shapes, and works with no endpoint
  running: failures are recorded per case and the table still prints.
- P1 GATE — the routing decision is NOT resolved. The spec's ~85% and ~70%
  thresholds concern Gemma 4 E4B on a GTX 1080. Neither was available: this
  machine is an RTX 5090 with 32GB and no Gemma 4 E4B is installed, so the
  measurement that sets the router's defaults still has to be run on the target
  box. Until it is, P5 should treat its thresholds as provisional.
- P1 evidence that the harness discriminates, taken on the RTX 5090 against what
  Ollama had: qwen3.6:27b scored 100% (50/50, every category, multi-turn
  included); llama3.2 scored 18% (9/50), failing every multi-turn case by
  answering in prose after step one and calling tools on 9 of 10 refusal cases.
  A harness that separates those two is measuring the right thing.
- Measurement caveat for P5: decode tok/s is unreliable against Ollama, which
  buffers short tool-call replies into one burst, leaving a decode window of a
  millisecond or two. Cases with a window under 50ms are excluded rather than
  reported as thousands of tokens per second. Trust the end-to-end figure.
- A warm-up request runs before the suite; without it the first case absorbed a
  33 second model load. Disable with `--no-warmup`.
- `xos-bench-results.json` is the default output and is not in `.gitignore`.
  P1's guardrail forbids touching files outside xos-bench/, so a later task that
  legitimately edits `.gitignore` should add it.

- P2 — done. Check passed: `cargo run --bin xosd` starts, writes
  `~/.config/xos/config.toml` on first run, and a JSON-RPC `health` call over the
  socket returns `{"status":"ok"}`. Verified end to end alongside halt, resume,
  status, an unknown-method refusal and a restart.
- Socket: `/run/xosd.sock` is the configured default. When that directory is not
  writable, which is every developer run, the daemon falls back to the runtime
  directory and logs a warning naming the new path rather than moving silently.
  The CLI tries the same candidates in the same order, so no flag is needed.
- Halt is an `AtomicBool` plus a broadcast channel, never a mutex, so a
  completion holding a lock cannot delay it. Order is flag, then waiters, then
  disk, so a slow disk cannot either. `Halt::guard()` is the call every later
  subsystem must make before acting; `rpc::complete` already uses it, and a
  completion in flight is cancelled through `Halt::subscribe()`.
- Every connection gets its own task, which is what makes halt dependable: a
  halt never queues behind a running completion.
- Streaming protocol for P3: `complete` emits `complete.delta` notifications
  carrying `{id, text}`, then the final result with text, finish reason and
  usage. The TUI can render token by token without a second protocol.
- Prefix caching: `cache_key` pins to a llama.cpp slot and sends `cache_prompt`
  with `id_slot`. `slots = 0` disables it, which is correct for Ollama, so
  `supports_prefix_cache` reports false there. P8's compiled prompts need a
  llama.cpp server started with slots to get the latency win.
- `ProviderError` carries only Transport and Http. Speculative variants were
  removed rather than silenced; add them when a caller needs one.
- The CLI treats a closed pipe as a normal end, so `xos status | head` does not
  panic. The first run of the check found that by piping into head.

- P3 — done. Check passed against a live model: the TUI streams a reply token by
  token and the status line shows a live figure, 164.8 tok/s during generation
  and 258.2 tok/s after. Verified by driving the real binary inside a pty and
  reading the escape codes back: box rules render in `#454340`, the local tier in
  `#7A9B6E`, both exact from STYLE.md, and no fourth hue appears.
- The TUI holds no model logic, per the guardrail. It calls `status` once for the
  provider and its tier, then every message is a `complete` call over JSON-RPC.
  xosd was not modified.
- The live rate counts `complete.delta` notifications, since the daemon emits one
  per token. The exact count from `usage.output_tokens` replaces the estimate when
  the reply finishes, so the figure settles to the truth rather than drifting.
- Testing a ratatui program in a pty needs TIOCSWINSZ set explicitly. A forked pty
  defaults to 0x0 and ratatui renders nothing into no space, which reads exactly
  like a hung interface. Worth knowing for P11 and P12, which also draw.
- The local config now points at `llama3.2:latest` rather than `gemma4:e4b`, since
  that is what this machine has. That is in `~/.config/xos/config.toml`, not the
  repository, so it affects nothing for anyone else.

- P4 — done. Check passed: `xos vault add openrouter` stored a key, `xos vault
  list` showed the provider name and nothing else, and a completion ran against
  a provider whose key came from the vault. Cost arithmetic verified by hand:
  31 input and 6 output tokens at 0.15 and 0.60 per 1k is 0.00825, which is what
  was recorded.
- A canary key was traced through the whole run. It appears in no CLI output, no
  daemon log, no config file, and not in the vault file, which is age ciphertext
  at 0600. The daemon is the only process that reads key material; the CLI sends
  a key in and never gets one back.
- No keyring answered in WSL, so the age-encrypted file backend was the one
  exercised. Its identity file sits beside the vault at 0600, so the protection
  is file permissions plus encryption at rest. That is written down in the module
  docs rather than implied, and the daemon logs which store is in use.
- Caps are checked before a request starts, never during one. Over a cap the
  provider is refused up front with -32002 and a sentence saying what to do. A
  reply already streaming is never interrupted by a cap.
- Verification limit: with no real cloud key available, the OpenAI-compatible
  provider was pointed at the local Ollama endpoint, which ignores the bearer
  token. That exercises vault to provider to completion to spend end to end, but
  it does not prove a remote endpoint accepts the header. The Anthropic provider
  is unit-tested for its event shapes, system prompt handling and required
  max_tokens, and has not been run against the real API.
- P3's TUI reaches cloud providers unchanged, since it asks the daemon which
  provider is default and renders the tier colour from the capabilities.
