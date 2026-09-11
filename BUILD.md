# XOS — Build Status

| # | ID | Task | Status | Gate |
|---|---|---|---|---|
| 1 | P0 | Repository scaffold | done | |
| 2 | P1 | XOS Bench — tool-call harness | done | *GATE* |
| 3 | P2 | xosd skeleton, provider trait, halt primitive | done | |
| 4 | P3 | xos chat TUI | done | |
| 5 | P4 | XOS Vault and cloud providers | done | |
| 6 | P5 | Router and escalation log | done | *GATE* |
| 7 | P6 | XOS Memory, export and import | done | |
| 8 | P7 | Policy engine and egress protection | done | *GATE* |
| 9 | P7b | Action journal and undo | done | *GATE* |
| 10 | P8 | Supervisor | done | |
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

- P5 — done. Check passed: all eight triggers were forced against a live model
  and each appears in `xos escalations` with the correct trigger and a reason
  that names the numbers. Throughput and low-confidence fired mid-stream, the
  other six before the request left.
- P5 GATE — the thresholds are placeholders until P1's measurement runs on the
  target box. `tokens_per_second_floor` 6.0, `min_mean_logprob` -1.0,
  `tool_complexity_threshold` 3 and `context_headroom` 0.8 are reasoned guesses,
  not measurements. A Gemma 4 E4B run on the GTX 1080 should set them, and the
  ~85% and ~70% tool-call figures decide whether the defaults hold at all.
- The check found a real flaw rather than confirming the code. Low confidence
  was firing on the first token, because the throughput trigger needs a window
  of at least a second before a rate can be computed and so fell through to the
  logprob check, which had a sample of one. Escalating on one token's
  probability is a coin flip, not a measurement. Confidence now needs
  `min_logprob_samples` tokens, default 8, before it can fire at all.
- Ollama does return logprobs on its OpenAI endpoint, so trigger 6 is exercised
  against real probabilities rather than a stub.
- Mid-stream escalation stops the local reply and re-runs on the API tier,
  emitting a `complete.escalated` notification first. Known rough edge: P3's TUI
  does not yet act on that notification, so a mid-stream escalation shows the
  abandoned local fragment followed by the API reply. P3 is a completed task and
  the driver forbids editing it here; P11 or P12 should handle the notification.
- `xos mode` writes to a state file rather than rewriting `config.toml`, so a
  runtime change never reformats a hand-edited file or drops its comments. The
  config supplies the default; the state file overrides it.
- Routing lives only in the router module. Providers were not told about tiers,
  and the Provider trait is unchanged. `Token` gained an optional `logprob`,
  which is the only way the model's own probabilities can reach the router.

- P6 — done. Check passed: three entries written to working memory, the task
  closed, the local model summarised them into session, working left empty, and
  `xos memory search` returned the summary. Export and import were exercised
  too: the bundle is age ciphertext at 0600, re-importing it added nothing and
  kept what was already here, a wrong passphrase was refused, and a fresh
  machine restored the memory and searched it.
- DEVIATION from the prompt: sqlite-vec is not used. Vectors are stored as f32
  BLOBs and cosine similarity is computed in Rust over the rows of the tier
  being searched. The reason is that sqlite-vec is a loadable extension, which
  means shipping and loading a native library on every target machine including
  the decade-old ones, for a corpus that is thousands of rows on a personal box
  where a linear scan is microseconds. Revisit when a real corpus makes the scan
  measurable; the storage format does not have to change to do it.
- The default embedder is lexical: hashed character trigrams, in process, no GPU
  contention, stable across runs. It matches wording, not meaning. It will find
  "ssh key path" from "ssh key" and will not find it from "the thing I log in
  with". A remote OpenAI-compatible embeddings endpoint can be configured when
  recall quality matters more than having no moving parts, and `xos memory stats`
  names which is in use so the limitation is never invisible.
- Promotion quality is the local model's quality. llama3.2 produced a usable
  summary but embellished it with advice nobody asked for. On the target box
  this is Gemma 4 E4B's job, and it is worth re-reading a promoted summary once
  before trusting the tier.
- Export carries provider names, never key material, by construction: the bundle
  type has no field a key could travel in, and import tells you which providers
  to reconnect. Import merges and never overwrites, so restoring an old bundle
  onto a live machine cannot roll it backwards.
- Scheduled export is off by default. When enabled it needs both a path and a
  passphrase in the environment variable named in config; if either is missing
  the daemon says so once at startup rather than silently writing nothing.

- P7 — done. Check passed on both halves. A read of `~/.ssh/id_rsa` blocks, in
  tilde and absolute form, and so do `.aws`, `.pem` and `.env`. A fake key was
  put in a temp file and handed to a cloud provider; it did not reach the API.
- P7 GATE — the egress half was proved on the wire, not by unit test. A
  capturing HTTP endpoint stood in for the cloud API and recorded exactly what
  the daemon sent: `[redacted: assigned-secret]` in place of the key, with the
  surrounding config intact. That is the assertion worth trusting, because
  testing the scanner alone would only prove the scanner runs, not that the
  transport calls it.
- The two axes are judged independently and blocks win. Loosening strictness to
  permissive relaxes prompting only; a secret path stays blocked and egress is
  still scanned. There is a test that holds that line.
- Verb matching had a real bug the tests caught: `draft_reply` prompted, because
  "reply" is irreversible. It is not the action — the leading word is. Drafting
  a reply is reversible and sending one is not, and prompting for the draft
  would have trained people to click through prompts, which is how a capability
  firewall stops working.
- The audit log records the spans it removed rather than a placeholder. The
  first run logged "0 credential-shaped strings removed" after removing one; a
  security record that undercounts is worse than none.
- Policy log arguments are redacted before they are stored, so the log does not
  become the thing worth stealing.
- `Permit` is the guardrail: a tool may only run when the engine has issued one,
  and `authorises` carries the debug assertion. It has no caller yet because XOS
  has no tool executor yet, which is why it is marked allow(dead_code) with the
  reason in the source. The first executor, in P9, must call it on every call.
- Unverified: the local SearXNG rule is enforced and tested, but no SearXNG
  instance exists here to route to.

- P7b — done. Check passed: XOS reorganised a test directory through its own
  tools, `xos undo` reversed it, and the directory came back with identical
  names, sizes, modes, modification times and contents. A second undo correctly
  found nothing left.
- P7b GATE — the check caught a real ordering bug. Timestamps have one-second
  resolution, and four actions inside one second tied, so undo removed the
  directory before moving the files back out of it. Reverse-chronological has to
  mean insertion order, not clock order, so ordering is now by row id. There is a
  test that pins it, because a passing clock cannot be relied on to expose it.
- A tool executor was added in `tools`, because P7b's check requires XOS to
  actually reorganise a directory and there was nothing that touched a disk yet.
  It runs four steps in order: policy judges, a permit is issued, the journal
  snapshots the prior state, and only then does the action run. That is also
  where P7's `Permit` guardrail finally has a caller, so no filesystem action can
  reach a disk without a decision behind it.
- Snapshot sharpness worth knowing: a hardlink preserves content when the
  original is unlinked or renamed, which covers deletes and moves, but not when
  it is overwritten in place, because both names share one inode. Size and mtime
  are recorded with the link and undo refuses a snapshot whose stats have moved
  rather than restoring the wrong bytes quietly. Files up to 256KB are copied
  inline instead, where this cannot arise.
- Journalling never causes a failure. A snapshot that cannot be taken is logged,
  tagged unreversible, and the action proceeds. Policy refusal does stop work,
  which is the difference between the two.
- An undo that cannot finish does not start: every step is checked before any is
  applied. A half-reversed directory is worse than one left alone with an
  explanation.
- Not yet wired: retention prunes by age on startup. The 2GB ceiling is recorded
  in config but not yet enforced, since nothing here can produce that volume.

- P8 — done. Check passed: two wakes with nothing listening queued rather than
  erroring, `supervisor.flush` processed both once an endpoint appeared, a first
  compile was adopted and the second identical task type reported not-due from
  the cache, and a deliberately worse candidate scored 0.00 against the
  incumbent's 1.00 on real cases and was rejected with both scores logged.
- The regression guard re-scores the incumbent on the same cases before
  comparing, so the decision is like for like rather than against a figure from
  another day. That re-measurement is bookkeeping and no longer writes a history
  line: the first run logged a spurious "rejected 1.00 to 1.00" that made the
  audit trail read as if a decision had been made when none had.
- A replacement that cannot be scored is refused rather than adopted, because
  silent drift is exactly what the guard exists to stop. A first compile has
  nothing to regress from, so it is adopted and marked unguarded, and
  `xos supervisor prompts` shows which prompts are in that state.
- The supervisor can only be handed a digest: `wake` takes one and there is no
  parameter for raw memory or raw tool output. The digest is re-scanned
  immediately before sending and a wake carrying anything credential-shaped is
  refused. `vet` returns rather than panicking so the refusal is testable and a
  release build still refuses; the send path carries the debug assertion.
- Recompiling changes the `cache_key`, which is what evicts the stale prefix from
  llama.cpp's slot, and resets the hit and failure counts, because old failures
  belong to the old prompt.
- Cost classes throttle independently: an expensive trigger going quiet does not
  silence a cheap one, and manual is never throttled. Over the daily ceiling the
  supervisor defers and says work stays local, rather than failing.
- Unverified: prompt compilation was exercised against a stand-in endpoint, not a
  real cloud model, so the quality of a compiled prompt is untested. The
  machinery around it — caching, scoring, adoption, rejection — is.
