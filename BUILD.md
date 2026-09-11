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
