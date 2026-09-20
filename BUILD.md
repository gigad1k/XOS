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
| 11 | P9 | XOS Goals, the task graph | done | |
| 12 | P10 | XOS Pulse and power management | done | |
| 13 | P11 | Status bar and desktop theme | done | |
| 14 | P12 | Mission Control | done | |
| 15 | HW-1 | Hardware detection | done | |
| 16 | HW-2 | Driver resolution and installation | done | *GATE* |
| 17 | P13 | Installer | done | *GATE* |
| 18 | P14 | First-run experience and first goal | done | |

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

- P9 — done. Check passed: a goal was decomposed by the supervisor into three
  dependency-ordered steps, the nodes ran on the local model, xosd was killed
  with SIGKILL part-way through, and after restart the graph was consistent and
  carried on to done.
- A deadlock was found and fixed before the check could run at all. `plan` still
  held the graph mutex when it called `refresh_blocked`, which takes the same
  lock; std's Mutex is not reentrant, so it hung. It hung the tests here, and it
  would have hung the daemon on the first goal anyone created.
- Recovery is on open as well as on demand: a node left `running` by a crash is
  set back to `pending`, because a process that died is not still working. The
  live kill happened to land between nodes rather than inside one, so the
  recovery path itself is proven by unit test rather than by that run.
- Conditions hold a node back rather than failing it. A node wanting AC power on
  a machine running on battery simply is not eligible, so it does not burn its
  retry budget on something that was never wrong. Anything unreadable is treated
  as permitting work, because a desktop has no mains supply to read and refusing
  to work there would be worse.
- The tier split is enforced by structure, not discipline: the graph module holds
  no provider handle, so a node cannot reach a model without going through the
  daemon, which applies the policy engine and the router first. Decomposition and
  re-planning are supervisor work; execution and summarising are local.
- A re-plan replaces unfinished nodes and keeps finished ones, so re-planning a
  goal does not throw away work already done.
- Model quality note, not a code issue: llama3.2 answered the third step with
  something about renewable energy. On the target box this is Gemma 4 E4B's job,
  and a node's result is worth reading before trusting it.

- P10 — done. Check passed in substance: a scheduled task fired from the
  heartbeat and became a goal, and a conditioned node deferred rather than
  failing. With the GPU deliberately loaded to 90% a node requiring `gpu-idle`
  stayed `pending` and the daemon reported nothing eligible; once the GPU fell to
  1% the same node ran and finished, with its retry budget untouched.
- Deviation from the check's wording: it says to unplug ethernet. That is not
  something this run could do, so the same property was demonstrated with the
  condition that could be controlled here, by loading the GPU. The network
  condition now reads the same machine state the model reads, so offline or
  metered both hold a node back, and the interval logic is unit-tested.
- Also worth stating plainly: an interval task is due the first time it is seen,
  so a task set to every two minutes runs on the first tick and every two
  minutes after. The two-minute wait in the check was therefore the heartbeat
  arriving, not the interval elapsing.
- Power management is wired, not decorative. The model is released from VRAM
  after an idle timeout, energy is sampled every tick from the GPU's own power
  reading plus a configured baseline for the rest of the box, and
  `xos pulse status` shows watt-hours and their cost beside API spend. In this
  run that read 7.3 Wh against 2.31 of API spend. Both numbers are real; the
  electricity one is labelled an estimate everywhere it appears, because the
  baseline is configured rather than measured.
- A year of idling at 80W comes to roughly £170 at 24.5p per kWh, which is the
  figure the prompt warns about. There is a test asserting that arithmetic, so
  the claim in the docs cannot drift away from the code.
- Suspend-with-RTC-wake is implemented as a command the caller may run and is off
  by default. `pulse.status` reports what would run rather than running it,
  because suspending a machine should not be a side effect of asking how it is.
- Pulse orchestrates only. It produces a list of actions and the daemon carries
  them out through the same path a person's request takes, so a node started by
  the heartbeat passes the policy engine and the router exactly as one started by
  hand. A halted system still ticks, so status stays truthful, and advances
  nothing.

- P11 — done. Check passed: the bar renders as waybar JSON, showed 190 tok/s
  live during a generation and dropped it afterwards, turned red with "1 need
  you" when a node reached needs-user, opened the expand panel, and went to the
  stop colour reading "halted — click to resume" when halted.
- The check found a real gap on the way. Node execution was judged as the
  literal tool `execute_node`, which is always allowed, so a node could never
  reach needs-user at all. Nodes are now judged on what the step actually does,
  so "delete the old release" prompts exactly as the tool call would. The state
  machine had a state nothing could enter, which is worse than a state that
  behaves wrongly, because nothing fails visibly.
- Quiet when idle is enforced in code, not just intended: with nothing running,
  nothing spent and nothing blocked, the bar reads "local" and nothing else.
  There is a test for it, because a bar that is always busy becomes wallpaper and
  then goes unread on the one occasion it matters.
- Colour is meaning only. There are exactly four classes — local, api, blocked,
  halted — and a test asserts that a fifth would mean a fourth hue. Red appears
  only for work needing a person or spend over its cap.
- The kill switch is bound in Hyprland to the CLI rather than to any running
  application, so it works when a UI is not.
- Shipped alongside: waybar config and stylesheet, Hyprland rules with 3px
  radius and the local green active border, a Plymouth theme, terminal palettes
  for Alacritty and foot, and an Open WebUI stylesheet. The terminal palettes map
  the non-semantic ANSI slots to the neutral ramp rather than inventing five more
  hues.
- The Open WebUI stylesheet leaves the name and logo visible, and says why in a
  comment: their licence requires it above 50 aggregate users. Restyling is fine;
  passing it off as XOS is not.
- Verification limit: there is no compositor on this machine, so waybar,
  Hyprland and Plymouth rendering are unverified. What was verified is the data
  the bar renders from and every branch that decides what it says.

- P12 — done. Check passed: a three-node goal ran, states updated live over
  `/api/state`, a node reaching needs-user was approved from the UI, and the
  graph then advanced to completion with the goal reaching done.
- The check found the bug that mattered. An approval did not stick: policy
  re-evaluated on the next attempt, prompted again, and the node returned to
  needs-user however many times it was approved. The state machine looked right
  and the button did nothing. A node now carries the approval, one yes covers
  one attempt, and the approval is spent when it is used.
- It also found that approving a node that does not exist returned success.
  Mission Control would have shown an approval that never happened, which is
  worse in a transparency surface than anywhere else. Unknown nodes, and nodes
  not actually waiting, are now refused with a reason.
- Read-only except for two things, as the guardrail requires: approving a blocked
  node and setting the cost mode, plus the halt control, which is the one thing
  that must always be reachable. Everything else is a window.
- The page holds no business logic. `/api/state` assembles one document in the
  daemon and the page renders it; there is a test asserting no pricing or routing
  logic appears in the page.
- Colour compliance is tested rather than trusted: every six-digit hex in the
  page is checked against the STYLE.md tokens, so a fourth hue fails the build.
- Served on loopback only. It is a window onto this machine and has no business
  being reachable from elsewhere. The HTTP surface is hand-rolled and small
  enough to read in one sitting, which matters for something that exposes the
  system's state.
- Verification limit: no browser here, so the page was exercised through its
  endpoints rather than rendered. Layout and motion are unverified; the data and
  every action are not.

### HW-1 — Hardware detection

- Read-only in the strongest sense the guardrail asks for. Everything comes from
  `/proc`, `/sys`, or a tool that itself only reads. Nothing is installed,
  loaded, modprobed or written. The check confirmed it: the set of loaded kernel
  modules was byte-identical before and after, and the daemon log contains no
  install-shaped line. The router and provider registry were not touched.
- `hardware-db.json` is the community table, sharing its shape with
  `verified-models.json` so both go through the same submission path. NVIDIA
  device-ID ranges map to architecture and driver branch; every row carries a
  confidence and, where the reasoning is not obvious, a `why`.
- Range order in the file is load bearing and says so: the Volta IDs sit inside
  the Pascal range, so Volta is listed first and wins. A test asserts it, because
  sorting the ranges at load time would silently break it.
- The check passes. `10de:1b80` resolves to Pascal, branch 580,
  `nvidia-580xx-dkms`, confidence confirmed, with `nvidia_drm.modeset=1` and a
  fallback chain. Kepler, Fermi, Tesla, Volta, Ada, an AMD card and an unknown
  card were all resolved as well.
- The check found five real defects, all now fixed:
  1. **Firmware said BIOS when it could not tell.** The absence of
     `/sys/firmware/efi` was being read as "legacy boot", but in a container, a
     VM or WSL there is no firmware to see at all. The installer partitions on
     this answer, so answering BIOS there would have laid an MBR on a machine
     that boots UEFI. There is now a third state, `unknown`, decided by whether
     DMI is visible, and it is a different fact that the installer must treat
     differently.
  2. **Detection depended on `lspci`.** pciutils is not installed on this
     machine, and an install image — exactly the machine that most needs its
     graphics card identified — very often has none either. PCI now comes from
     `/sys/bus/pci/devices`, which is the kernel and is always there. `lspci` is
     still used when present, but only to put readable names on devices the
     kernel has already reported.
  3. **The fallback chain offered branches that cannot drive the card.** It was
     built by walking every other branch, which reads as helpful and is the
     opposite: exactly one proprietary branch drives any given card, and the
     others do not partly work on it, they do not work at all. Offering 470 to a
     Pascal owner whose 580 install just failed costs them another reboot and
     gives them a wrong theory about why. The chain is now the open driver, then
     a plain framebuffer.
  4. **VRAM was read for "a" GPU rather than for this one.** Both the nvidia-smi
     and the sysfs path took the first answer they found, so a second card would
     have been reported with the first one's memory — on the machine most likely
     to care. Both now match on the PCI address.
  5. **An unknown device passed off its loaded module as a driver.** The card
     here reported `dxgkrnl`, which is a fact about this machine and not
     something anyone can install; HW-2 reads that field as a package name and
     would have failed on it. An unknown device now says it is unknown, and what
     is driving it is reported separately as the fact it is.
- `xos hardware --device 10de:1b80` answers for a card that is not in the
  machine. That exists because someone planning an install, or helping a stranger
  through a chat window, needs the answer before owning the hardware — and it is
  how this check was run without a GTX 1080. The output says plainly that the
  card is not present and that nothing was changed to answer.
- `profile()` is deliberately conservative: a GPU needs 4 GB of VRAM before XOS
  claims it can run a model locally, and below 8 GB of system memory it says
  api-only rather than promising slow CPU inference. Promising local inference a
  machine cannot deliver makes a bad first hour.
- The CPU check confirms baseline x86-64 rather than v2 or v3, as the prompt
  requires, and reports the level it actually found. A machine below baseline is
  told so before an install, not after.
- Verification limits, plainly. There is no GTX 1080 here, so the reference
  mapping was verified through the database and the resolve path rather than
  against the card; the code that reads a real NVIDIA card's VRAM ran only on the
  no-match path. This is WSL, which has no firmware, no PCI display device and no
  DRM outputs, so `uefi` and `bios` were not observed on real firmware — only
  `unknown` was, which is the correct answer here and was itself the defect the
  check found. Secure Boot reading, EDID-backed display enumeration and SMART are
  unexercised.
- 290 tests pass across the workspace; the build carries no warnings.

### HW-2 - Driver resolution and installation

- `install/00-hardware.sh` runs before every other install step. There is no
  `set -e` in it, deliberately and with a comment saying so, and the harness
  asserts its absence: one failing command must never abort an install.
- Every path ends somewhere that shows a picture. The graphics chain runs
  matched driver, then the database's fallback list, then `modesetting`, then
  `vesa` - and the last two are not packages at all, which is why the chain
  cannot run out. A test drives it with a package manager that refuses
  everything and asserts the result is still a working display mode.
- The guardrail is kept exactly: a device that is not in hardware-db.json gets
  no driver. The log says "not in hardware-db.json, so no driver is guessed at"
  and the chain takes over.
- Running the real log, rather than trusting the harness, found four defects
  the tests had not been written to catch:
  1. **The kernel module was installed and its userspace was not.** The database
     has carried a `utils` package per branch since HW-1 and nothing read it. A
     machine would have booted with a healthy `nvidia` module, no libGL and no
     Xorg driver - which is a black screen, the single outcome this file exists
     to prevent. The resolution now carries `utils`, the installer installs and
     pins it with the driver, and a failure to install it fails the whole level
     so the chain moves on.
  2. **The report claimed the bluetooth service was enabled two lines under
     "could not enable bluetooth.service".** The report is what someone reads
     when the machine misbehaves months later, so it now says what happened
     rather than what was attempted.
  3. **The AMD kernel parameters were unreachable.** They sat on a fallback path
     that only ran when the database driver failed, and it does not fail. A GCN
     1.0 card would have fallen to radeon silently and the machine would just
     have been slow. `amd_generations` in hardware-db.json now maps device IDs
     to GCN 1.0 and 1.1, the parameters ride on the recommendation, and the card
     reports its generation so the parameters beside it are not mysterious.
  4. **Dead code** left in the graphics loop, removed.
- A fifth defect came from the harness itself: a later edit dropped a closing
  `fi` and the script stopped parsing. The failures it produced looked like six
  unrelated problems. `bash -n` is now the first thing the harness checks,
  because an installer that does not parse is the worst failure available and it
  should be reported as one thing rather than six.
- The submission report is built in the daemon, once. The install script asks
  `xos hardware --json` for it rather than assembling its own, because two
  descriptions of what would be sent is one too many: the day someone adds a
  field to one, the other keeps quietly promising the old contents.
- What the report leaves out is the point, and a test enforces it: no hostname,
  no user name, no MAC address, no serial number, no SSID. A subsystem ID of
  0000:0000 is dropped too - that is the kernel saying there is no subsystem,
  and a database keyed on it would collect a meaningless row.
- `xos hardware --submit` exists because the install log tells people to run it.
  A command promised in an install log and then missing is its own small breach
  of trust. It prints the whole report, asks, and only then sends - and the
  send goes through the daemon, where the policy engine judges the egress and
  the policy log records it, like any other.
- The check passes. On the unsupported-wifi machine the install completes with
  exit 0, graphics settle on the first choice, and the log carries a boxed,
  unmissable "WIFI IS NOT AVAILABLE ON THIS MACHINE" with a specific suggested
  fix and the sentence "The install is continuing. The desktop will boot."
- 341 tests pass across the workspace and 36 in the installer harness; the build
  carries no warnings.

#### Gate review - HW-2

What a human should verify, and why, before the installer in P13 is trusted with
a real disk:

1. **The NVIDIA branch table against real cards.** `hardware-db.json` maps device
   ID ranges to architectures and branches. Only the GTX 1080 row is marked
   `confirmed`; everything else is `known`, meaning taken from documentation and
   not observed here. A wrong row is a black screen on someone's machine. The
   Volta range deliberately precedes the Pascal range because it sits inside it -
   that ordering is load bearing and a test pins it.
2. **The AMD GCN 1.0 and 1.1 ranges**, added in this task and entirely
   unobserved. These decide whether an old Radeon is driven by amdgpu or falls
   to radeon.
3. **That a real install actually boots.** Nothing here can prove it. The script
   was exercised against a fake root and a fake package manager; no package was
   installed, no module loaded, no disk touched. "The desktop boots" was verified
   only by proxy: a graphics level is always settled and the chain terminates at
   a mode that works on any VGA hardware.
4. **The submission endpoint.** `https://hardware.xos.community/submit` does not
   exist yet. Nothing sends to it without an explicit yes, and the failure path
   is handled and reported, but the address is a placeholder that someone has to
   make real or change.
5. **The bundled DKMS sources.** The script looks in
   `/run/archiso/bootmnt/xos/dkms`. P13 has to actually put rtl8821ce, rtl8723bu,
   broadcom-wl and relatives there, because the AUR needs the internet that those
   drivers exist to provide.

Deliberate scope decision, recorded per driver.txt: this task wrote the
installation code and did not run it. No driver was installed, no package
manager invoked, no kernel module loaded and no disk written on the machine this
was built on. Everything was exercised against a temporary root with a stub
package manager.

### P13 - Installer

- `install/` now holds `boot.sh`, `base.sh`, `install.sh`, `lib.sh`,
  `packages.lock`, the numbered steps 01 to 09, and a harness of 91 tests
  beside HW-2's 36.
- Nothing Omarchy owns is modified. `layer_into` refuses to overwrite a file it
  did not write; `include_once` adds exactly one idempotent `source =` line to
  hyprland.conf and nothing else. Waybar has no include directive, so the XOS
  bar lives in its own directory with a README saying how to use it, and
  Omarchy's config is left byte-identical - which the harness checks by hashing
  it before and after.
- Every package is pinned in `packages.lock`, and `pin_install` refuses anything
  not in it. A test walks every script, collects every package name, and fails
  if one is unpinned.
- The amendment is implemented. `base.sh` detects firmware and partitions
  accordingly: GPT with an EFI system partition under UEFI, MBR with a BIOS boot
  partition and GRUB to the MBR under legacy. That path is the reason XOS ships
  its own base installer at all, since Omarchy's requires UEFI and would exclude
  most pre-2012 machines.
- `base.sh` is the only file in XOS that destroys data, and it behaves like it:
  it prints the disk and its contents, says plainly that everything on it will
  be destroyed, requires the disk name to be typed back, refuses outright when
  nothing is attached to the prompt, and does nothing at all under `--dry-run`.
  It also refuses to partition when it cannot tell UEFI from BIOS rather than
  guessing, since guessing produces a machine that installs cleanly and then
  does not boot.
- LUKS is opt-in and AES-NI is detected. Without AES-NI the script says
  plainly that encryption will make the machine noticeably slower and asks
  again, because many target machines predate AES-NI and full-disk encryption on
  them turns a slow machine into an unusable one.
- Socket activation, which is the architecturally significant part: Open WebUI,
  SearXNG and the OpenClaw gateway are each a socket unit, a
  `systemd-socket-proxyd` with `--exit-idle-time=15min`, and the real daemon
  behind it with `StopWhenUnneeded=yes`. The sockets hold the ports from boot at
  no cost; the services exist only between the first connection and fifteen idle
  minutes later. Only xosd and llama-server are started at boot. The exception is
  implemented and commented: with messaging configured the OpenClaw gateway is
  enabled at boot, because an inbound message cannot socket-activate a listener
  that is not listening.
- `09-xosd.sh` stands alone. Run by itself on an existing Arch or Debian box,
  with no other XOS file present, it defines its own helpers, detects the
  package manager, builds, installs and enables the unit. A test copies it
  somewhere with no `lib.sh` beside it and runs it.
- Reading the actual install log, rather than trusting the harness, found seven
  defects. Four of them mattered:
  1. **A real install would have installed nothing and reported success.**
     `install.sh` passed `${XOS_DRY_RUN:+--dry-run}` to every step, and `:+`
     expands whenever the variable is non-empty - which "0" is. Every step was
     always told to do nothing. The log read perfectly, and somebody would have
     found out at the reboot. It now tests the value, and there is a test that a
     run without `--dry-run` actually installs something.
  2. **`09-xosd.sh` ran the host's real `apt-get` during a test.** It hardcoded
     the package manager instead of honouring `XOS_PACKAGE_MANAGER`, so it
     reached straight past the substitution. It failed only because the tests
     were not run as root. It now uses whatever was substituted, and the harness
     stubs pacman, apt-get, apt, dpkg, pacstrap and arch-chroot as well, because
     a test suite must not be able to install packages on the machine running it.
  3. **`09-xosd.sh` announced "xosd is installed" directly after two failures to
     install it.** The same defect as HW-2's bluetooth line and P7's audit log,
     in a place somebody acts on. It now says what happened and exits non-zero.
  4. **Every memory threshold excluded the machine it was written for.** A
     machine sold as 8GB reports around 7800 MB, because firmware and an
     integrated GPU take their cut before Linux sees any of it, so
     `minimum_memory_mb: 8192` matched no 8GB machine at all - and the spec says
     in as many words that XOS targets 8GB machines. Every threshold in
     verified-models.json is now written against what a machine reports rather
     than what it was sold as, and a test fails any threshold that is a round
     power of two.
  The other three: `07-models.sh` reported "nothing fits" when it had simply not
  been able to read the hardware report, which is a different fact and a
  discouraging thing to tell somebody untruthfully; the socket units carried a
  `BindIPv6Only` line that does nothing on an IPv4 literal and implied a
  protection it was not providing; and a dropped `fi` from an earlier edit.
- **`verified-models.json` was created here, and no prompt owns it.** HW-1's
  prompt refers to it as an existing thing to copy the shape of, and P13's
  07-models.sh reads it, but nothing creates it. Recorded per driver.txt as an
  ambiguity resolved: P13 ships it, because a step that reads a file nobody
  creates is the same broken promise HW-2 had. Gemma 4 E4B is rank 1 and the only
  `confirmed` row, matching the spec.
- Verification limits, plainly. The check P13 asks for is a clean Arch VM, and
  there is not one here. Nothing was installed, no disk was partitioned, no
  bootloader written and no machine rebooted. Everything was run against a
  temporary root with the package manager and every external command stubbed.
  So: every script parses and runs, the ordering is right, the layering is
  right, the units say what they should, and whether the resulting machine
  actually boots into a working desktop is exactly what has not been shown.

#### A defect in HW-1, recorded rather than fixed

`MIN_CPU_MEMORY_MB` in `xosd/src/hardware/mod.rs` is 8192, and a machine sold
as 8GB reports around 7800. A machine with 8GB and no usable GPU is therefore
reported as `api-only` when it should be `cpu`, so XOS would tell the owner of
its own headline target machine that it cannot run a local model. It is the same
mistake as the model thresholds, in the task next door. `MIN_LOCAL_VRAM_MB` at
4096 is on the same edge for a 4GB card.

driver.txt says not to edit a file belonging to a completed task, and to write
it in Notes instead, so that is what this is. It is a two-constant change and
worth making before P14.

#### Gate review - P13

What a human should verify, and why:

1. **A clean Arch VM, which is the actual check.** Run `base.sh --dry-run`
   first, then for real on a disk that can be lost, then `install.sh`, then
   reboot. Everything below this line has been tested; this has not.
2. **Both firmware paths on real hardware.** The UEFI path and the legacy BIOS
   path produce different partition tables and different bootloader installs,
   and the BIOS path is the reason this file exists. Neither has booted a real
   machine.
3. **The pinned versions.** `packages.lock` holds versions that were current for
   Arch when it was written. Arch moves. A stale pin degrades the install rather
   than breaking it, since `pin_install` reports what it could not get and
   carries on, but somebody should refresh them against a current mirror.
4. **The socket activation, under load.** Fifteen minutes of idle before a
   service stops is a guess about how people use Open WebUI. Worth watching on a
   real 8GB machine before anyone trusts the number.
5. **`https://xos.sh/boot` and the repository URL in boot.sh** are placeholders
   pointing at this repository. Neither is real yet.
6. **The bundled DKMS sources HW-2 expects at
   `/run/archiso/bootmnt/xos/dkms`.** Still nothing puts them there. That is an
   ISO-building job, which no prompt in this build covers, and without it the
   unsupported-wifi fallback in HW-2 has nothing to try.

Deliberate scope decision, again recorded: this task wrote the installer and did
not run it. No disk was partitioned, no bootloader written, no package installed
and no service enabled on the machine this was built on.

### P14 - First-run experience and first goal

- `xos setup` is the wizard: nine steps, in the order the prompt gives them,
  every one skippable. `--skip-all` takes the default for everything, which is
  what the installer uses and what the check runs.
- Skipping all nine is a supported path with its own test, and the finish says
  so in as many words: "You skipped all of it, and XOS still works. It runs on
  this machine, offline, with no account and no key." The person who most needs
  that sentence is the one who skipped every step.
- The rendering is a pure function over the wizard state, the way the status bar
  is, so all nine steps are tested without a terminal.
- Things the steps do rather than say:
  - The hardware step lists what is **not** working and why, including a card
    that is not in hardware-db.json. Left out, somebody finds it a week later
    and has no idea it was known about on day one.
  - The model step shows the fit in numbers - what the model wants against what
    the machine has - because a forty-minute download that turns out not to fit
    is a bad first hour.
  - The policy step describes what each choice permits in plain words and never
    shows the mode names. Somebody in their first minute has no idea what
    "balanced" means.
  - Messaging puts both warnings before the offer: use a dedicated number, and
    an allowlist is required before it turns on at all.
  - The kill switch step asks somebody to press Ctrl+Alt+Escape and actually
    halts and resumes the daemon when they do. A control nobody has used is one
    they will not reach for at the moment they need it.
- The first goal is built in and fixed, written out rather than planned by a
  model: three nodes, each waiting on the one before, so Mission Control shows
  it working rather than three nodes turning green at once.
- It is read-only in the strong sense. `firstrun/scan.rs` lists directories and
  opens nothing, and there is a test that greps its own source for
  `read_to_string`, `File::open` and `fs::read` and fails if any appears. Every
  listing goes through the policy engine, so a directory somebody has marked
  private is invisible to the first goal exactly as it is to everything else.
  A missing or unreadable directory is skipped without a word.
- What goes into memory is about the shape of the work, never a filename. There
  is a test that puts "tax return 2024.pdf" and "divorce settlement.pdf" in the
  sample and asserts neither reaches the summary. The guess about what somebody
  works on is always marked as a guess, because it is inferred from filenames and
  a confident wrong statement about a person is a worse first impression than
  none at all.
- Mission Control has the matching view, reading the same `firstrun.describe`
  and running the same goal through the same code, so somebody who set up in a
  terminal and somebody who set up in a browser end in the same place.
- `models.recommend` was added to the daemon so the wizard and the install script
  read verified-models.json through one rule rather than two. It reports what did
  not fit and why, with the numbers, because somebody wondering why they were not
  offered the big model deserves the answer.
- The check passes: the wizard completes with every step skipped, the first goal
  runs and writes one entry to long-term memory, and `xos chat` answers offline
  on the local tier with no API key configured anywhere. Running the wizard a
  second time does not run the goal again.
- Two defects found by running it:
  1. **`xos setup | head` aborted the wizard partway.** `print!` panics when the
     other end of the pipe has gone, so the machine was set up and the one goal
     that demonstrates it was silently skipped - the worst possible version of
     that bug, because everything looked fine. All output now goes through a
     writer that treats a closed pipe as a closed pipe, and a test greps the
     module for `print!` and `println!` to keep it that way.
  2. **The 8GB thresholds from HW-1, which this task had to fix rather than
     record.** See below.

#### A file from a completed task was edited, and why

`MIN_CPU_MEMORY_MB` in `xosd/src/hardware/mod.rs` was 8192 and
`MIN_LOCAL_VRAM_MB` was 4096. No machine sold as 8GB reports 8192 MB, and no
card sold as 4GB reports 4096, because firmware takes its share before Linux
sees any of it. The effect was that a real 8GB machine was reported `api-only`,
and P14's own model step then had nothing to offer it - on the machine the
specification names as the target.

This was recorded at the P13 gate as something to fix rather than fixed, per
driver.txt. It then broke this task's check, so it is fixed here: 7600 and 3800,
with the reasoning written next to the constants and two tests pinning it. The
deviation from driver.txt is deliberate and recorded, which is what that rule
asks for when an earlier task turns out to be wrong.

#### A gap in the specification, recorded rather than filled

The reason given for choosing this first goal is that it "seeds memory so the
second interaction is already personalised". It does seed memory: the entry is
written, `xos memory search` finds it, and the long-term tier reports one entry.

But nothing recalls it into a conversation. Asked "what do you know about this
machine?" immediately afterwards, XOS answers that it does not know - correctly,
because the `complete` path never touches memory. `memory.recall` exists and is
exposed over RPC and the CLI, and nothing calls it while answering.

No prompt in this build asks for that. P3 specifies `memory.recall` as an RPC and
a CLI search and nothing more, and its guardrail says not to change the router,
which is where conversational context is assembled. So this is a gap in the
specification rather than a defect in any task, and filling it would mean
building something no prompt describes.

It is the single most valuable thing to do next. Until it exists, memory is a
filing cabinet that XOS never opens while talking to you, and the first goal's
stated purpose is only half true.

#### Verification limits

No GTX 1080, so the reference hardware path is still unobserved. The wizard was
run headless with every step skipped and driven through a pty for the chat step;
the interactive branches of the steps that shell out - `gh auth login`, `rclone
config`, the WhatsApp QR - were not exercised, because each opens somebody else's
login flow. Mission Control's first-run view was exercised through its endpoints,
not rendered in a browser.

### The install medium

Not a numbered task — this is the ISO that HW-2 and P13 both assumed and neither
built, added after the build was otherwise complete.

- `iso/` is an archiso profile: `profiledef.sh`, `packages.x86_64`,
  `pacman.conf`, the live filesystem overlay, syslinux for legacy BIOS and
  systemd-boot for UEFI, plus `build.sh` and 71 tests.
- Both firmware paths are boot modes, with BIOS listed first. That ordering is
  not cosmetic: it is the mode most likely to be forgotten and the one this
  project cannot do without, since Omarchy's own installer requires UEFI and
  would exclude the machines XOS exists for.
- Both firmwares also get a basic-graphics entry with `nomodeset`. A machine
  whose card the kernel cannot drive must still be installable, and that entry
  is the difference between a black screen and an install.
- `install/live.sh` is the all-in-one installer, and `install-xos` on the medium
  is one word that runs it. It reads the machine, lists the disks, asks which,
  asks about encryption and a name, then drives base.sh and install.sh and
  offers to reboot. The only thing it refuses to do without a person is choose
  the disk.
- **The medium carries prebuilt `xos` and `xosd`.** Driver resolution lives in
  the daemon and the CLI is a thin client over it, so on live media with no
  daemon the hardware step would have found no inventory and resolved no drivers
  at all — on exactly the machines that most need it. `live.sh` starts the
  daemon, uses it, and stops it.
- **The bundled wifi drivers finally exist.** `build.sh` fetches, builds and
  places rtl8821ce, rtl8723bu and broadcom-wl on the medium. HW-2 has been
  reaching for these since it was written; until now nothing put them there.
- Writing it found two real faults:
  1. **The drivers would never have been found.** `00-hardware.sh` looked only in
     `/run/archiso/bootmnt/xos/dkms`, which is the mounted image; archiso's
     overlay lands the files in the live filesystem instead. The bundled drivers
     would have been built onto the medium and then ignored. It now searches the
     places they can actually be, in order.
  2. **The package list had a name that does not exist** (`wireless_urch_tools`),
     which fails the entire image build. That is exactly the class of fault the
     profile tests were written to catch before somebody spends half an hour
     discovering it.
- A typo in a package name, a boot entry naming a kernel path the profile does
  not produce, a bootmode with no configuration behind it: each costs a full
  build to find and each is visible without one. `iso/tests/iso.test.sh` checks
  all of it, and checks that what `build.sh` writes is where `00-hardware.sh`
  looks.
- **Verification limit, and it is the whole of it.** `mkarchiso` runs on Arch and
  there is no Arch machine here, so the image has never been built, written to a
  stick, or booted. The profile is consistent and its tests pass; whether it
  produces a medium that boots on a real UEFI machine and a real BIOS machine is
  exactly what has not been shown. `install/live.sh` was exercised end to end
  with every external command stubbed: it completes, refuses to choose a disk
  unattended, and calls nothing destructive under `--dry-run`.

### Building and booting the medium

The ISO existed as a profile and had never been built. An isolated Arch
instance was made for it — a separate WSL distribution imported from the
official Arch image, with its own filesystem — because mkarchiso only runs on
Arch. archiso 90, qemu and OVMF went in beside it, so the image could be booted
as well as built.

**Everything below was found by building and booting, not by reading.** Before
the first build the profile passed 73 of its own tests. It was unbootable.

- **Every path in the syslinux configs was wrong.** ISOLINUX resolves a
  relative path against the directory holding isolinux.bin, which archiso puts
  at `/boot/syslinux`, so `boot/syslinux/whichsys.c32` asked for
  `/boot/syslinux/boot/syslinux/whichsys.c32`. The machine stopped at a bare
  `boot:` prompt. archiso's own profile uses bare filenames; so does this one
  now.
- **The kernel path had no leading slash and no install directory**, so it
  resolved under `/boot/syslinux/` like everything else and pointed at nothing.
- **The BIOS menu had no DEFAULT and no TIMEOUT**, so a machine left to install
  itself waited forever for a keypress.
- **The profile shipped no `splash.png`** while the menu named one.
- **The initramfs had no archiso hook.** The profile shipped no
  `mkinitcpio.conf.d/archiso.conf` and no `mkinitcpio.d/linux.preset`, so
  mkinitcpio built a stock initramfs that knows nothing about live media. The
  bootloader and kernel worked perfectly and then Switch Root failed and the
  machine sat in an emergency shell. **A bootloader menu proves the bootloader
  and nothing else.**
- **The root account was locked**, so the emergency shell could not even be
  used: "Cannot open access to console." archiso expects a getty autologin
  drop-in, which this profile did not have.
- **Arch's first-boot wizard ran instead of XOS.** With the boot fixed, the
  medium came up asking for a timezone. Somebody who booted a stick to install
  XOS was three questions deep in an Arch setup wizard.
- **`install-xos` shipped non-executable.** mkarchiso does not preserve modes
  from the profile's airootfs — everything arrives 644 — and only paths named
  in `file_permissions` get what they need. The one command the medium tells
  people to type could not run. So could not `live.sh`, which is why the
  launcher now goes through `bash`.
- **`build.sh` silently shipped a medium with no XOS on it.** Its fallback used
  rsync, which is not a dependency it checked for; when rsync was missing it
  printed "command not found", said the source had been copied, and carried on.
- **A second build did nothing and reported success.** mkarchiso marks finished
  steps inside its work directory and skips them next time, so rebuilding into
  the same one produced no image at all and exited 0.
- **The wifi drivers could not have been built.** `makepkg` ran as `nobody`,
  whose home is `/` and is not writable, with `-s` which needs root to call
  pacman.
- **A floppy was offered as an install target.** QEMU gives every machine an
  `/dev/fd0`, it reports itself as a disk, and it was listed first at 4K.
- Four bootmodes used spellings archiso 90 deprecates, and the package list
  named `wireless_urch_tools`, which does not exist and fails the whole build.

The audit that ran alongside this raised 58 findings across the profile, the
build script and the install path, and independently reached every one of the
faults above. It also reached three the boot could not: **the XOS layer
installed into the live medium's RAM rather than onto the target disk**, so the
machine would have rebooted into bare Arch; **an encrypted install got neither
the encrypt hook nor a cryptdevice parameter**, so it could not have unlocked
itself; and **`base.sh` discarded every exit status**, so a failed pacstrap
carried on writing to the live filesystem.

#### What is actually proven

- The image builds on Arch, twice over, from a clean work directory.
- It boots on **legacy BIOS** and on **UEFI**, both to a root shell, with the
  XOS motd on screen and the hostname set.
- Typing `install-xos --dry-run` on the booted medium starts the installer,
  which reads the machine, says plainly what it could not learn, lists the
  disks and asks which one to use.

#### What is still not

- **Hardware detection during the install is not working yet.** (Since fixed, and
  the reason given here turned out to be wrong — see "The daemon was never slow;
  it was never asked" below.) The binaries are
  on the medium and executable, and the daemon does not come up inside the live
  environment within the time the installer waits. The install falls back to
  resolving drivers conservatively, which is what that path is for, so it is a
  degradation rather than a failure. It also reported the wrong reason: one
  message covered "no binaries", "daemon would not start" and "inventory
  unreadable", and it sent somebody looking for a missing file that was there.
  Three messages now, and the daemon's log is named.
- No install has been carried through onto a disk and rebooted from. The
  installer was exercised as far as the disk question.
- The bundled wifi drivers have never been built: that step reaches the AUR,
  and a network failure there would say nothing about the profile.
- No physical machine has run any of this. Everything above is QEMU.

### Booting straight into the installer, and a headless manual

- The medium now starts the installer by itself. Somebody who put a stick in a
  machine to install XOS should not then have to be told a command to type.
- **The login file never ran.** It was `.zlogin`, which is zsh, and there is no
  zsh on the medium — root's shell is bash. The motd that appeared on screen was
  pam_motd doing it, not the profile. Renamed to `.bash_profile`, and there is a
  test now that fails a login file written for a shell the medium does not carry.
- The destructive step is still asked for. The default entry starts the
  installer and the installer still shows the disk and waits for its name to be
  typed back.
- Unattended installs exist and are chosen: a third boot menu entry on both
  firmwares, labelled "Install XOS automatically (erases the largest disk)",
  passing `xos.auto` on the kernel command line. It counts down for twenty
  seconds in front of whoever is standing there, and Ctrl+C stops it. Sorted
  last on UEFI so it is never what pressing Enter lands on.
- Both paths were verified by booting: touching nothing reaches the installer's
  first question, and choosing the unattended entry reaches the countdown with
  the disk already chosen.
- `docs/headless.md` covers running XOS with no desktop — the daemon and a local
  model on a box reached over SSH, Mission Control and llama-server forwarded
  rather than exposed, what is resident and what it costs, and the fact that the
  kill switch is a keybinding that does nothing over SSH so the command matters.
  `docs/manual.test.sh` checks its commands too.

### Carrying an install through onto a disk

The one claim never tested was the one that matters to somebody holding a USB
stick: that an install completes and the machine boots from its own disk
afterwards. Everything before this stopped at the disk question. Running it
found three defects, two of them by reading the code on the way and one only by
watching the screen.

#### The two that a boot could not have found

- **An encrypted install wrote a boot parameter nothing read.** base.sh
  generates grub.cfg while setting up the bootloader, and the encryption block
  runs after it. It added `cryptdevice=UUID=...` to `/etc/default/grub` and
  stopped: the config the firmware actually reads still described an
  unencrypted machine. The initramfs got its encrypt hook, so the machine would
  have installed cleanly, rebooted, found no cryptdevice on its command line
  and had nothing to unlock — the exact failure the comment three lines above
  it warns about. It now regenerates, and checks the parameter went in at all
  first, because the sed that writes it silently does nothing if
  GRUB_CMDLINE_LINUX is not where it expects.
- **The hardware step's kernel parameters never reached the boot menu.** Same
  defect one layer out, and worse, because it is on every install rather than
  the encrypted ones. 00-hardware.sh resolves what this machine's card needs
  and writes it to `/etc/xos/kernel-parameters` — but it runs as part of the
  XOS layer, which runs after base.sh has already generated grub.cfg. Nothing
  read the file back. A card whose resolution asks for `nvidia-drm.modeset=1`
  and does not get it comes up to a black screen, which is the single outcome
  00-hardware.sh exists to prevent. `10-boot.sh` now runs last, after
  everything that could ask for a parameter, and is the only thing that
  regenerates the boot config with the answer.

Both are the same mistake: writing to a file after the thing that reads it has
already run. A search for a third turned up nothing at the time - nothing
outside base.sh touches mkinitcpio.conf, fstab or the GRUB defaults - but that
search only covered files written after a reader. A third instance of the wider
fault, a step running against the wrong root, was found later and is recorded
at the end of these notes.

#### The one only a boot could find

- **A failed install restarted itself forever, erase and all.** Taking the
  unattended entry, the installer reached "Looking at this machine", stopped,
  and the whole thing began again from the boot banner, every couple of
  minutes. agetty respawns the login when its shell exits, and `.bash_profile`
  is that shell; it ran `exec install-xos --auto`, so the installer *was* the
  session. When it exited the session ended, root was logged back in, and the
  installer started again. Nothing bounded it. On the default entry that is an
  annoyance; on the unattended entry it is a machine that picks the largest
  disk, erases it, and on any failure does the whole thing again indefinitely
  with nobody watching.

  A marker in `/run` — tmpfs, so empty on every boot — now means it starts once
  per boot, and a reboot still starts over. And no `exec`: an installer that
  exits under exec leaves no shell to read the log that would say why, which is
  why this needed a screenshot rather than a log to find.

#### The daemon was never slow; it was never asked

The notes above said hardware detection during the install "does not come up
inside the live environment within the time the installer waits", and put it
down to the daemon being slow on live media. That was wrong, and the wrongness
was load-bearing: it made a bug look like a performance characteristic, and
performance characteristics do not get fixed.

Running the medium's own `xosd` by hand settles it in one line. It comes up in
a third of a second and logs `xosd listening socket=/run/xosd.sock`. The client
beside it, from the same medium, says `cannot reach the daemon. Tried:
/run/xosd-probe.sock`.

`XOS_SOCKET` was only ever read by `xos-cli`. `live.sh` exports it to put both
ends in the same place, which moved the client and left the daemon on its
configured path. Any session with `XDG_RUNTIME_DIR` set — which is every
autologin under systemd, so every boot of the medium — has the two ends looking
at different paths. The installer then waits its fifteen seconds for a socket
that will never exist and reports a daemon that is running fine.

`resolve_socket` reads the variable now, `resolve_socket_with` takes it as an
argument so the test does not have to mutate the environment of every test
beside it, and an override that cannot be honoured names the variable in its
reason. The two hardware calls in `live.sh` are bounded at thirty seconds as
well: the daemon being up and the daemon answering are different things, and
there is nobody at an unattended install to press Ctrl+C.

The general lesson is the one this whole file keeps recording. Running the
thing found what the tests could not, and running the *component* by hand found
in one line what reading the logs had misdiagnosed for a week.

#### The install that erased the disk and then could not install anything

The unattended install partitioned `/dev/vda`, made both filesystems, mounted
them, downloaded all 550MB of the base system, and then stopped:

```
warning: Public keyring not found; have you run 'pacman-key --init'?
downloading required keys...
error: keyring is not writable          (twenty times)
error: required key missing from keyring
==> ERROR: Failed to install packages to new root
```

The downloads above the error are the important part: the network was never the
problem. Arch packages are signed, this medium had no initialised pacman
keyring, and on a squashfs root there was nowhere writable to build one.

Arch's own releng profile carries two units for exactly this — a tmpfs mounted
over `/etc/pacman.d/gnupg`, and a oneshot bound to it that runs `pacman-key
--init` and `--populate` at boot. This profile was written without them, so
every install it has ever attempted would have failed at the same line, and
would have failed with the disk already erased.

Three earlier fixes are what made this findable at all. Without the restart-loop
fix the installer erased and retried forever and never showed the message;
without dropping `exec` there was no shell to read the log from; and the log
only mattered because the socket fix had already cleared away a misdiagnosis
that would have sent the next hour in the wrong direction.

#### One more step that thought it was on the machine it was building

Reading ahead to what runs after pacstrap found the same fault a third time.
Every line of `install.sh` is `$XOS_ROOT`-aware except the one that installs the
desktop, which was a pipe into bash on whatever machine happened to be running
the script:

```
curl -fsSL https://omarchy.org/install | bash
```

During an install from the medium that put the entire desktop into the live
system's RAM. The machine would then reboot into the target disk, find Arch with
the XOS layer on top and nothing able to draw a window, and the copy that did
get installed is discarded by the same reboot that revealed the problem.

An audit had already found this exact defect for the XOS layer and it was fixed
there. It was left one step above, in the line that installs the thing XOS is
layered over.

It downloads into the target and runs under `arch-chroot` now, because a pipe
cannot cross a chroot. The un-chrooted form is kept for installing onto a
machine you are already sitting at, which is the case it was written for and the
only one where it is correct.

So: three separate instances of one mistake — a step that writes to, or runs on,
the wrong root or the wrong moment. The earlier claim in these notes that a
search found no third instance was made before this one was looked for, and it
was wrong. Ordering and targeting are now checked by tests in all three places,
because prose in a comment did not stop any of them.

#### It installs, and the machine boots from its own disk

With the keyring units in place the unattended install ran through: it chose the
largest disk, wrote an MBR with a BIOS boot partition, made and mounted both
filesystems, pacstrapped Arch, installed GRUB, and handed over to the XOS layer,
which read the machine and resolved its display driver (`bochs-drm` to `mesa`,
confidence `known` — which also demonstrates the daemon working during an
install, the thing these notes previously recorded as not working).

Then it stopped and asked:

```
Send it? [y/N]
```

and waited, on a machine deliberately left alone. That prompt is guarded by "is
stdin a tty", and a console is a tty whether or not anybody is sitting at it.
Both halves are fixed: the answer can arrive through `XOS_ASSUME_NO`, which
`live.sh` exports whenever it was told not to ask anything, and the question is
bounded so a console nobody is reading cannot hold an install open. An expired
question sends nothing.

**The disk that install produced was then booted on its own, with no medium
present:**

```
Arch Linux 7.2.6-arch2-1 (tty1)

xos login:
```

That is the claim this project had never tested. An install carried through onto
a disk, and a machine that boots from it afterwards, with the hostname the
installer was told to set.

What is proven and what is not, precisely: the base install completes and boots.
The XOS layer had started and had finished its hardware step when this run was
stopped at the prompt above; the desktop, the local model and the daemon's own
units have not been carried through on a real disk yet. And still no physical
machine: all of this is QEMU with KVM.

#### The file that prevents black screens was causing one

Reading the rest of the layer for the same fault found it twice more, and the
second is the worst thing in these notes.

`09-xosd.sh` ignored the binaries the medium carries and rebuilt from source.
`install_build_dependencies` runs `pacman -S rust` directly rather than through
the target-aware wrapper beside it, so during an install from media a Rust
toolchain is fetched into the live system's RAM and then ten minutes of
somebody's install go on recompiling what is already sitting in
`/usr/local/bin` — on a machine chosen for being old, into a tmpfs that an 8GB
box may not have room for. It now prefers what is already built whenever there
is a separate root to install into, and says so plainly when a medium carries
none.

`00-hardware.sh` was worse. It works out which driver this card needs — the
entire point of the file — and then installed it with a bare `pacman -S`.
Everything else in that file is ROOT-aware: the log, the report, the kernel
parameters, the pacman pins. Not the one call that puts the driver somewhere.
Installing from media, the driver went into the live system in RAM, and the
reboot discarded it. The machine came up on its own disk with no driver at all,
which is the black screen the file's own header calls the one unrecoverable
outcome.

The query was the same mistake pointing the other way: `pacman -Qq` asked the
medium whether a package was installed, and the medium carries `linux-firmware`,
`networkmanager` and plenty else the fresh target does not, so those were
reported "already here" and skipped.

A sweep of every other step found none: 01 through 08 and 10 install only
through `pin_install`, which goes through the wrapper.

So the count for the day is five separate instances of one mistake — a step
acting on the machine doing the building rather than the machine being built —
in `base.sh`, `install.sh`, `09-xosd.sh` and twice in `00-hardware.sh`. None of
them were caught by 300-odd shell tests, and all of them are now.

#### The install reported success and produced a machine missing most of XOS

Sweeping the layer for the same fault turned it from three instances into eight,
and the shape of the last five is worth stating plainly because they are the
most dangerous kind of bug in this codebase: **they all failed politely.**

02-runtimes installs pipx, npm and uv into the target. Five later steps then
asked `command -v` whether those tools existed — on the machine running the
script, which during an install from media is the live system, where they never
were. So each skipped itself and said something entirely reasonable:

```
pipx is missing, so Open WebUI was skipped
npm is missing, so the OpenClaw gateway was skipped
npm is missing, so OpenCode was skipped
uv is missing, so piper was skipped
no huggingface CLI, so nothing was downloaded
```

None of them were missing. They were on the disk being built, one `arch-chroot`
away. An install from the medium therefore produced a machine with no chat
interface, no search, no messaging, no coding agent, no speech, no wake word and
no local model — and finished by saying it had installed XOS.

The last of those breaks the first non-negotiable in CLAUDE.md, which is that
XOS works offline. A machine with no local model does not.

`lib.sh` already had `package_manager()`, which exists precisely to cross into
the target, with a comment saying it is "the difference between installing XOS
and installing nothing". It solved the problem for packages and only for
packages. `in_target()` now sits beside it and does the same for everything
else, and it is a no-op when XOS is installed onto the machine you are sitting
at — the case all fourteen bare calls were written for.

The test greps every layer step for a bare probe or a bare call; against the
commit before the fix it names four files.

#### Proven end to end

On a medium built from every fix above, with nothing touched after choosing the
unattended entry:

- it picked the largest disk, wrote an MBR with a BIOS boot partition, made and
  mounted both filesystems;
- `pacstrap` succeeded, with no keyring error;
- `arch-chroot /mnt grub-install --target=i386-pc /dev/vda`, and "Base install
  finished. Arch is on /dev/vda and it boots bios";
- the XOS layer ran against `root: /mnt`, the target, not the live medium;
- the hardware step read the machine through the daemon and resolved its display
  driver;
- it rebooted itself, unmounting the `/etc/pacman.d/gnupg` tmpfs on the way out.

And the disk it produced, booted on its own with no medium anywhere near it:

```
Arch Linux 7.2.6-arch2-1 (tty1)

xos login:
```

What is still not proven: no physical machine has run any of this, the bundled
wifi DKMS drivers have never been built because that step reaches the AUR, and
the layer's optional services were not individually verified on the installed
disk — only that they now run against the right root, which is what every one of
them was getting wrong.
