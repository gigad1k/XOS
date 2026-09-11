# XOS — Build Prompts

Run in order. Each is self-contained, restates its context, protects prior work, and ends
with a check you can run. Copy the code block, paste into Claude Code, verify the check
passes before moving on.

## P0 — Repository scaffold

```
Set up the XOS repository scaffold.

Context: XOS is an AI-native Linux OS. The core is `xosd`, a Rust daemon. This is a
fresh repo with nothing in it yet.

Create:
- Cargo workspace at root with members: xosd, xos-cli, xos-bench
- xosd/ as a binary crate with modules: providers, router, memory, policy,
  scheduler, state, graph, vault, supervisor (empty mod.rs files with doc comments
  describing each subsystem's responsibility)
- xos-cli/ as a binary crate named `xos`
- xos-bench/ as a binary crate
- .gitignore covering: target/, *.gguf, *.db, config/openclaw/config.json,
  .env, any *.key or *.pem
- README.md with a hardware compatibility table, first row:
  "Ryzen + GTX 1080 8GB | Gemma 4 E4B | confirmed working"
- LICENSE (MIT)

Guardrail: create files only, do not add dependencies beyond what compiles an empty
workspace.

Check: `cargo build` succeeds and `cargo run --bin xos` runs without panicking.
```

## P1 — XOS Bench, tool-call harness

```
Build XOS Bench, the model validation tool, in the xos-bench crate.

Context: XOS routes between a local model and a cloud API. The decisive metric is
tool-call schema success rate, not tokens/sec — a fast model that emits malformed
tool calls forces constant escalation and costs API rates anyway. Gemma 4 E4B is
confirmed running on a GTX 1080 via Ollama and LM Studio; this measures whether it
is reliable enough to drive tools.

Build a harness that:
- Connects to an OpenAI-compatible endpoint (llama.cpp server or Ollama) at a
  configurable base URL
- Loads a suite of test cases from JSON: each has a prompt, a tool schema, and an
  expected-shape assertion
- Ships 50 built-in cases across: single tool call, multi-turn sequence, tool call
  with nested object args, tool call requiring a numeric argument, correct
  refusal-to-call when no tool applies
- Records per case: schema-valid, arguments-plausible, latency, tokens in/out
- Reports: tool-call success %, multi-turn completion %, mean tok/s, time to first
  token, VRAM at peak if nvidia-smi is available
- Outputs a human table to stdout and a JSON result file
- `--submit` flag: no-op stub for now, prints where results would be sent

Guardrail: read-only against the model endpoint. Do not modify any files outside
xos-bench/ and the output result file.

Check: `cargo run --bin xos-bench -- --url http://localhost:11434/v1` produces a
table with a tool-call success percentage.
```

**Run this against Gemma 4 E4B before continuing.** Above ~85% tool-call success, the
local-first architecture holds as designed. Below ~70%, the router escalates constantly
and the supervisor tier needs to carry more work — same architecture, different defaults.

## P2 — `xosd` skeleton and provider trait

```
Build the xosd daemon skeleton and the provider abstraction.

Context: xosd is the XOS intelligence layer. The architecture rule is that nothing
in XOS imports a model provider directly — everything goes through xosd. This
prompt establishes that boundary. See CLAUDE.md.

Build:
- A `Provider` trait with async methods: complete(request) -> stream of tokens,
  capabilities() -> struct describing context window, tool support, modality,
  cost per token, and whether it is local
- The request type carries an optional `cache_key: Option<String>`. Providers
  that support prefix caching keep that prefix's KV cache warm and skip
  reprocessing it on subsequent calls. This is load-bearing for latency — see
  CLAUDE.md — and the supervisor's compiled prompts depend on it (P8).
- A `LlamaCppProvider` implementing it against an OpenAI-compatible endpoint,
  using slot/prefix caching so a cache_key maps to a persistent slot
- A `ProviderRegistry` holding named providers, with get(name) and list()
- A Unix socket server at /run/xosd.sock exposing JSON-RPC methods:
  `complete`, `providers.list`, `health`
- Structured logging to stderr with tracing
- A config file at ~/.config/xos/config.toml, created with defaults on first run
- **A halt primitive.** JSON-RPC method `halt` that immediately cancels every
  in-flight completion, sets a global halted flag that all future subsystems must
  check before acting, and persists that flag so a restart does not silently
  resume. `resume` clears it. This is XOS's kill switch and it is built now,
  before there is anything to halt, so no subsystem is ever written without it.
  It must work even if the daemon is otherwise busy — handle it on a dedicated
  task, never behind a lock that a running completion could hold.
- CLI: `xos halt`, `xos resume`, `xos status`

Guardrail: no router logic, no memory, no policy in this prompt — those are
separate subsystems. Keep the provider trait free of any XOS-specific concepts so
future providers stay simple to add.

Check: `cargo run --bin xosd` starts, creates the config, and a JSON-RPC `health`
call over the socket returns ok.
```

## P3 — `xos chat` TUI

```
Build the xos chat TUI in the xos-cli crate.

Context: xosd is running and exposes JSON-RPC over /run/xosd.sock (see P2).

This TUI is XOS's **default interface**, not a fallback. It is instant, carries no
licence constraints, runs on hardware where Chromium struggles, and validates xosd
end to end with no web configuration in the way. Open WebUI is opt-in and spawns
only when asked for. Build this to a standard you'd be happy using daily.

Build with ratatui:
- Connects to /run/xosd.sock, fails with a clear message if xosd is not running
- Streaming chat view: user messages and assistant responses, token-by-token
- Bottom status line showing: active provider name, local or api, live tokens/sec
  during generation
- Ctrl+C exits cleanly, Ctrl+L clears

Style per STYLE.md: IBM Plex Mono, three semantic colours only — green for local
tier, amber for api tier, oxide red for errors. Box-drawing rules in the line
colour. No other colour anywhere. Labels in sans-equivalent weight, values in mono.

Guardrail: the TUI must contain no model logic of its own. Every completion goes
through xosd's JSON-RPC. Do not modify xosd in this prompt.

Check: with xosd running and a llama.cpp server up, `xos chat` streams a response
and the status line shows a live tokens/sec figure.
```

## P4 — XOS Vault and cloud providers

```
Build XOS Vault and add cloud API providers.

Context: xosd has a Provider trait and registry (P2). XOS supports any AI provider
via user-supplied API keys. Keys must never touch disk in plaintext or enter git.

Build:
- A `Vault` module storing credentials in the system keyring via the `keyring`
  crate, with a fallback to an age-encrypted file if no keyring is present
- `vault.set(provider, key)`, `vault.get(provider)`, `vault.list()` — list returns
  provider names only, never key material
- An `OpenAiCompatibleProvider` covering OpenRouter, OpenAI, Groq, DeepSeek,
  Together and any custom base URL
- An `AnthropicProvider`
- Per-provider spend tracking: tokens in/out and cost, persisted to SQLite,
  queryable by day
- Per-provider and global daily spend caps in config; exceeding a cap makes the
  provider unavailable rather than erroring mid-request
- CLI: `xos vault add`, `xos vault list`, `xos spend`

Guardrail: xosd must be the only process reading key material. Never log a key,
never include one in an error message, never return one over JSON-RPC. Do not
change the Provider trait from P2.

Check: `xos vault add openrouter` stores a key, `xos vault list` shows the provider
without the key, and `xos chat` can complete against it.
```

## P5 — Router and escalation log

```
Build the XOS router.

Context: xosd has local and cloud providers registered (P2, P4). The router decides
which tier serves each request. Local is preferred; the API is an escalation, not a
default. Every escalation must be auditable.

Implement escalation triggers, in evaluation order:
1. Schema validation failure — malformed tool call, retry local once, escalate on
   second failure
2. Task class — chat, summarise and lookup stay local; code generation and long
   multi-step work go to API
3. Context overflow beyond the local model's practical window
4. Timeout or tokens/sec falling below a floor, escalating mid-stream
5. Tool complexity — a heuristic on the number of chained tool calls a request
   implies
6. Logprob confidence — mean token logprob across the tool-call span below
   threshold. Use llama.cpp's logprobs; never ask the model to self-report
   confidence, as small models are badly calibrated
7. Cost mode — aggressive-local, balanced, or best-quality, from config
8. Manual escalation, never throttled

Log every escalation to SQLite: timestamp, trigger, task class, local model,
target model, tokens, cost, outcome.

CLI: `xos escalations` shows recent entries, `xos mode <mode>` sets cost mode.

Guardrail: routing decisions live only in the router module. Providers stay
unaware of routing. Do not modify the Provider trait.

Check: force each trigger with a crafted request and confirm `xos escalations`
records it with the correct reason.
```

## P6 — XOS Memory

```
Build XOS Memory, the three-tier persistent memory system.

Context: xosd needs memory that survives reboots and follows the user across chat,
terminal and messaging. SQLite plus sqlite-vec is the store; this prompt builds the
management layer on top.

Tiers:
- Working — current task. Minutes to hours. Active context, tool results, scratch.
- Session — current day. Conversation, recent files, today's decisions.
- Long-term — persistent. Preferences, projects, contacts, workflows, compiled
  prompts.

Implement:
- SQLite schema with sqlite-vec embeddings, stored at ~/.local/share/xos/memory.db
- A local embedding model for vector search (small, CPU, no GPU contention)
- Explicit promotion: working summarised into session at task close, session
  distilled into long-term at day close. Summarisation runs on the local model.
- `memory.recall(query, tier, limit)` returning ranked results
- `memory.write(tier, content, tags)`
- JSON-RPC exposure so all clients share one memory
- CLI: `xos memory search <query>`, `xos memory stats`
- **Export and import.** `xos export <path>` writes a portable, age-encrypted
  bundle containing memory, task graph, compiled prompts, config and vault
  metadata (never key material — the bundle records which providers were
  configured, not their secrets). `xos import <path>` restores it, merging rather
  than overwriting. The whole value of XOS is that it remembers you; a dead disk
  or a reinstall must not erase that. Build this alongside memory, not later.
- A scheduled export to a configurable path, off by default, opt-in in config

Guardrail: memory is local-only and never transmitted wholesale. Anything bound for
a cloud API must go through the digest builder added in P8, not raw recall. Do not
change the router.

Check: write to working memory, close the task, confirm a summary lands in session
and `xos memory search` retrieves it.
```

## P7 — Policy engine and egress protection

```
Build the XOS policy engine — the capability firewall.

Context: xosd executes tools on the user's behalf, including on input arriving from
untrusted sources like inbound messages. Nothing gets direct shell, filesystem or
browser access. Every tool call passes through this engine.

Two independent axes:

Axis 1 — reversibility:
- Irreversible (delete, send, purchase, publish, install, sudo) → prompt the user
- Reversible (read, search, summarise, draft, organise) → allow

Axis 2 — data egress. Reversibility alone is insufficient: reading ~/.ssh is
reversible in file terms, but once that content reaches a cloud API the secret has
left permanently.
- Reads of designated-secret paths → blocked outright regardless of reversibility.
  Paths: ~/.ssh, ~/.gnupg, keyring stores, password manager data, browser cookie
  and credential databases, .env files, *.pem, *.key, cloud credential directories.
- Any tool result entering API-bound context → scanned and redacted for
  credential-shaped strings before egress. This catches the unanticipated case: a
  repo the agent legitimately reads that contains a committed .env.
- Search in aggressive-local mode → routed to local SearXNG only.

Implement:
- A `PolicyDecision` enum: Allow, Prompt, Block, Redact(spans)
- `policy.evaluate(tool_call, context) -> PolicyDecision`
- A digest builder assembling bounded, redacted context packets for the supervisor,
  capped at 8k tokens
- All decisions logged to SQLite
- Config for strictness level, defaulting to the rules above
- CLI: `xos policy log`, `xos policy test <tool> <args>`

Guardrail: no tool execution path may bypass the engine. Add a debug assertion that
fails loudly if a tool executes without a PolicyDecision. Do not modify memory or
router internals.

Check: attempt a read of ~/.ssh/id_rsa and confirm it blocks; put a fake API key in
a temp file, have a tool read it, and confirm the string is redacted before it
would reach an API.
```

## P7b — Action journal and undo

```
Build the action journal.

Context: the policy engine (P7) prompts before irreversible actions. But
reversible actions can still be wrong — XOS reorganises a folder and makes it
worse, or edits a file badly. Prompting for those would be unbearable, so instead
they are recorded with enough state to reverse them. This is what makes it
possible to trust XOS with a filesystem.

Implement:
- A `journal` module recording every reversible action xosd takes: file writes,
  moves, renames, deletes, permission changes, config edits
- Each entry stores: timestamp, tool, arguments, originating goal or node, and a
  snapshot sufficient to reverse it. For small files store content; for larger
  ones or bulk operations use a hardlink-based snapshot into
  ~/.local/share/xos/journal/ to avoid duplicating data
- Configurable retention, defaulting to 7 days or 2GB, whichever comes first
- `journal.undo(n)` reversing the last n actions in order, transactionally — if
  any step cannot be reversed, stop and report rather than partially applying
- `journal.list()` with filtering by goal, time range and tool
- CLI: `xos undo`, `xos undo --last 5`, `xos journal`, `xos journal --goal <id>`
- Actions that genuinely cannot be journalled (network sends, external API
  effects) are tagged unreversible and must have gone through a P7 prompt

Guardrail: journalling must never be the reason an action fails. If a snapshot
cannot be taken, log it, tag the entry unreversible, and proceed. Do not modify
the policy engine — this sits alongside it, recording what it allowed.

Check: have XOS reorganise a test directory, run `xos undo`, and confirm the
directory returns to its exact prior state including timestamps where possible.
```

## P8 — Supervisor

```
Build the supervisor — the event-driven cloud tier.

Context: XOS runs two tiers of cognition. Gemma 4 E4B is the always-on reflex
layer. The cloud API is a supervisor that wakes on events, thinks hard, and sleeps
again. It is never continuous — a continuously-running supervisor would cost
£100-200/month and break both the offline guarantee and the egress protection built
in P7.

Wake triggers, each with a cost class for independent throttling:
- New goal (high) — full decomposition
- Node failure (high) — re-plan after retry budget exhausted
- Batch review (medium) — every N completed nodes, read digest and course-correct
- Prompt compile (medium) — new task type, or observed failure rate above threshold
- Confidence floor (low) — local model logprobs below threshold
- Novel situation (low) — nothing in memory matches
- Daily pass (medium) — scheduled planning and morning brief
- Manual (never throttled)

Prompt compilation is the highest-leverage function: the supervisor writes tight,
task-specific instructions for the local model — tool schemas, worked examples,
failure modes — keyed by task type and cached in long-term memory. Recompile when
that task type's failure rate crosses threshold. Compile once, execute hundreds of
times.

Implement:
- `Supervisor::wake(trigger, digest)` using the digest builder from P7
- A compiled-prompt cache in long-term memory, keyed by task type, with hit-rate
  and failure-rate stats
- Each cached prompt carries a stable `cache_key` passed to the local provider
  (P2), so llama.cpp keeps its KV cache warm and never reprocesses the prompt
  prefix. Compiled prompts plus cache reuse compound: better instructions and no
  cost for sending them. Evict a cache_key on recompile.
- Budget enforcement: daily token ceiling, per-trigger throttles, degradation to
  local-only when exhausted rather than failure
- **Regression guard on recompiled prompts.** Recompilation changes system
  behaviour silently, so a new prompt is never adopted on faith. Before swapping,
  run it against the relevant xos-bench cases for that task type and keep the new
  prompt only if it scores better. Log both scores. Without this, autonomy drifts
  and nobody can say when or why it got worse.
- Offline behaviour: cached prompts keep working, known task types keep executing,
  new decomposition queues until connectivity returns
- CLI: `xos supervisor log`, `xos supervisor prompts`

Guardrail: the supervisor may only ever see digests from P7's builder, never raw
memory or raw tool output. Add an assertion enforcing this. Do not modify the
policy engine.

Check: trigger a wake with the network disabled and confirm it queues rather than
errors; re-enable and confirm it processes. Confirm a compiled prompt is cached and
reused on the second identical task type.
```

## P9 — XOS Goals, the task graph

```
Build XOS Goals — the long-running task graph.

Context: this is what makes XOS an operator rather than an assistant. Users declare
goals; XOS decomposes them into tasks that execute over days. The graph persists in
xosd and survives reboots.

Structure: Goal → Task → Subtask → result, with dependencies between nodes.

Tier split (this matters — 4B models are poor at long-horizon decomposition):
- Goal decomposition → supervisor (API)
- Re-planning after failure → supervisor (API)
- Node execution → local model
- Result summarisation → local model

Node state machine:
pending → blocked → running → needs-user → done, with failed → re-plan

- blocked: waiting on another node's result
- needs-user: an irreversible action awaiting confirmation per P7
- failed: triggers supervisor re-plan, subject to a retry budget

Nodes may declare execution conditions checked against system state: on AC power,
GPU idle, unmetered network.

Implement:
- SQLite schema for goals, nodes, edges, state transitions and results
- `graph.create_goal(description)` → supervisor decomposes → persisted graph
- `graph.advance()` — called by the scheduler, runs eligible nodes
- `graph.status(goal_id)` and `graph.list()`
- CLI: `xos goal new "<description>"`, `xos goal list`, `xos goal show <id>`

Guardrail: node execution must route through the policy engine and the router. The
graph module must not call providers directly. Do not modify the supervisor.

Check: create a multi-step goal, confirm the supervisor decomposes it, confirm
nodes execute locally, kill xosd mid-run and confirm the graph resumes correctly on
restart.
```

## P10 — XOS Pulse

```
Build XOS Pulse — the scheduler and monitoring layer.

Context: Pulse is what lets XOS make progress while the user is away. It advances
the task graph, runs scheduled work, watches for events and raises notifications.

Implement:
- A tick loop with a configurable interval, defaulting to 5 minutes
- Graph advancement each tick, respecting node execution conditions
- Cron-style scheduled tasks defined in config
- File watchers triggering tasks on change
- A system state collector: GPU utilisation and VRAM, CPU load and temperature, RAM
  pressure, battery and AC, disk usage, network quality and metered flag, running
  applications, focused window, pending updates
- State exposed to the model so it can reason about the machine, e.g. deferring a
  large download on a metered connection
- Desktop notifications via mako for events needing attention
- A morning brief assembled from overnight activity
- CLI: `xos pulse status`, `xos pulse tasks`
- **Power management — this is not optional.** A GTX 1080 and an old PSU idling
  24/7 for the heartbeat draws roughly 60-100W, about £150-200/year at UK rates.
  "Free local AI" stops being free, which undercuts the entire pitch. Implement:
  unload the model from VRAM after a configurable idle timeout and reload on
  demand; let the GPU drop to its lowest power state between ticks; support
  suspend-with-RTC-wake so overnight scheduled work does not require staying up;
  estimate and record watt-hours consumed per day. Expose the estimate so Mission
  Control can show running cost beside API spend — that comparison is the honest
  version of the local-versus-cloud trade.
- Respect the halt flag from P2 on every tick. A halted system advances nothing.

Guardrail: Pulse orchestrates only — it must not execute tools directly or bypass
the policy engine. Do not modify the graph module's internals.

Check: schedule a task two minutes out, confirm it runs; unplug ethernet and
confirm a network-conditioned node defers rather than failing.
```

## P11 — Status bar and desktop theme

```
Build the XOS waybar module and desktop theme.

Context: the status bar is XOS's signature surface — the one place where boldness
is spent. It answers three questions at a glance: what is the system doing, what is
it costing, can I trust it. Everything else in the desktop stays quiet.

Build a waybar custom module reading from xosd's JSON-RPC:

Always visible:
- XOS button (leftmost) — click opens the command palette, right-click opens
  Mission Control
- Workspace indicators 1-4
- Current goal and node progress, e.g. "deploy site · 3/7", positioned centre-left
  where a taskbar would normally show windows
- Tier indicator: "local" in green, "api" in amber
- Spend today
- Clock

Contextual, appearing only when relevant:
- Live tokens/sec during generation
- Microphone state when listening
- Blocked-node count in oxide red — the only element permitted to use red
- Spend turning amber near budget, red over
- **Halted state.** When the system is halted, the entire bar shifts to the stop
  colour and reads "halted — click to resume". Unmissable, unambiguous.

Also bind the kill switch globally in Hyprland (suggest Ctrl+Alt+Escape, and make
it configurable). It calls `halt` over JSON-RPC and must work even when a UI is
unresponsive — bind it to the CLI, not to a running application.

Click expands a panel showing: model and quantisation, VRAM used of total, cost
mode, supervisor wakes today, Pulse status and last run, tokens today split local
versus api.

Design rule: quiet when idle, loud when it matters. A permanently busy bar becomes
wallpaper.

Also produce: Hyprland config (3px radius, 6px gaps, line colour inactive border,
local green active border), Plymouth theme, terminal ANSI palette, and Open WebUI
CSS — all from the STYLE.md tokens.

Guardrail: STYLE.md's three-colour rule is absolute — green means local, amber
means costing money, red means needs you. No fourth hue, no decorative colour. The
Open WebUI name and logo must remain visible; their licence requires it above 50
aggregate users.

Check: the bar renders, updates live during generation, shows a blocked count when
a node needs confirmation, and the expand panel opens.
```

## P12 — Mission Control

```
Build XOS Mission Control.

Context: autonomy without visibility feels unpredictable. Mission Control is the
transparency layer and it lives on workspace 4. Its primary view is the task graph,
which is the most important screen in the product.

Build as a local web UI served by xosd (Chromium is already running, so a native
app is unnecessary):

Primary view — goal graph:
- Live tree of goals, tasks and subtasks with node states colour-coded per
  STYLE.md's three hues
- Blocked nodes surfaced at top with their pending confirmations, actionable inline
- Failed nodes with their reason and re-plan status

Secondary panels:
- System state: GPU, VRAM, CPU, memory, disk, network
- Routing: recent escalations with trigger reasons
- Spend: today and this month, per provider, against caps — **shown beside
  estimated local running cost in watt-hours and currency**, so the
  local-versus-cloud trade is visible in the terms that actually matter
- Memory: tier sizes, recent promotions, last export
- Security: policy decisions, blocks, redactions
- Action journal: recent reversible actions with an undo control per entry
- Pulse: last tick, scheduled tasks, watchers, power state

A persistent halt control sits in the header, always reachable, styled in the stop
colour. When halted, the whole view takes the halted treatment from P11.

Style per STYLE.md: dense data rows, labels in sans left-aligned, values in mono
right-aligned with tabular numerals, 1px hairlines, 3px radius, no shadows, no
decorative colour. Motion only on node state change.

Guardrail: read-only except for approving blocked nodes and adjusting cost mode.
Mission Control must not contain business logic — every value comes from xosd's
JSON-RPC.

Check: run a multi-node goal, watch states update live, approve a blocked node from
the UI and confirm the graph advances.
```

## HW-1 — Hardware detection module

```
Build the hardware detection module in xosd.

Context: XOS targets old and varied PCs — Intel and AMD CPUs, NVIDIA, AMD and Intel
graphics, Realtek and Broadcom networking. Detection results drive three things:
driver selection at install, the model profile at runtime (local / cpu / api-only),
and the community compatibility database. See CLAUDE.md.

Build a `hardware` module that inventories:
- CPU: vendor, model, cores, threads, flags, microarchitecture level (must confirm
  baseline x86-64; XOS does not target v2 or v3)
- Memory: total, available
- GPUs: every PCI display device with vendor ID, device ID, subsystem ID, current
  kernel driver, VRAM where readable
- Network: ethernet and wifi devices by PCI/USB ID, current driver, firmware state
- Bluetooth, audio, storage (with SMART where available)
- Firmware: UEFI or legacy BIOS, Secure Boot state
- Displays: connected outputs and which GPU drives them

Ship `hardware-db.json` mapping PCI IDs to required drivers, firmware packages and
kernel parameters. Structure it exactly like verified-models.json so both are
community-updatable through the same submission path.

The NVIDIA table is the critical piece — map device IDs to architecture and branch:
Turing and newer to current, Maxwell/Pascal/Volta to 580, Kepler to 470, Fermi to
390, older to nouveau.

Also implement:
- `hardware.resolve()` returning, per device, the recommended driver plus an ordered
  fallback chain
- `hardware.profile()` returning local, cpu or api-only based on GPU capability
- Sources: lspci -nn, lsusb, dmidecode, /proc/cpuinfo, /sys, nvidia-smi when present
- CLI: `xos hardware`, `xos hardware --json`

Guardrail: detection is read-only. This module never installs, modifies or loads
anything — resolution only. Installation belongs to HW-2. Do not modify the router
or provider registry.

Check: `xos hardware` correctly identifies a GTX 1080 as Pascal requiring the 580
branch, and reports the firmware mode as UEFI or BIOS accurately.
```

## HW-2 — Driver resolution and installation

```
Build automatic driver installation for the XOS installer.

Context: HW-1 detects hardware and resolves what each device needs. This installs
it. XOS targets old PCs across a wide hardware range, so the governing rule is
absolute: never fail to boot. Every path ends at something usable — a degraded
desktop is recoverable, a black screen is not.

Build install/00-hardware.sh, running before all other install steps:

1. Run `xos hardware --json` for the inventory
2. Install linux-firmware and sof-firmware unconditionally
3. Graphics, walking the fallback chain until one succeeds:
   - NVIDIA: install the branch matched by device ID and pin it in pacman.conf so
     updates cannot break it. Fall back to open kernel modules, then nouveau, then
     modesetting, then VESA.
   - AMD: amdgpu, adding si_support/cik_support kernel parameters for GCN 1.0/1.1.
     Fall back to radeon, then modesetting.
   - Intel: i915, falling back to modesetting.
   - Hybrid graphics: drive the display from the integrated GPU, leave the discrete
     card headless for compute.
4. Networking:
   - In-tree drivers plus firmware first
   - If wifi is non-functional, try bundled DKMS sources (rtl8821ce, rtl8723bu,
     broadcom-wl and relatives — these must ship on the install media, because the
     AUR needs the internet that wifi is supposed to provide)
   - If still non-functional, prompt for USB tethering or ethernet, and offer to
     continue offline
5. Bluetooth: bluez plus firmware, enable the service
6. Audio: pipewire with wireplumber, verify a sink exists, log and continue if not
7. Write a detection report to /var/log/xos-hardware.log listing every device, the
   driver chosen, and which fallback level was reached
8. Offer to submit the anonymised report to the compatibility database — opt-in,
   never automatic, showing exactly what would be sent

Guardrail: no step may abort the install. Any failure logs, falls back, and
continues. Never install a driver whose device ID is not in hardware-db.json —
prefer a working fallback to a guess. Do not modify install scripts from P13.

Check: on a machine with an unsupported wifi chip, the install completes, the
desktop boots, and the log clearly states wifi is unavailable with a suggested fix.
```

## P13 — Installer

```
Build the XOS installer layer over Omarchy.

Context: XOS layers over Omarchy (Arch + Hyprland, MIT) and never forks it — owning
a distro would mean owning kernel updates, firmware and driver breakage. The
installer runs Omarchy, then applies the XOS layer.

Build install scripts in install/:
- 01-drivers.sh — NVIDIA 580 branch pinned (the last branch supporting Pascal;
  590 dropped it) plus CUDA. Pin in pacman.conf so updates cannot break it.
- 02-runtimes.sh — Node LTS with npm and pnpm, Python 3 with uv, Rust, Go, Podman,
  base-devel, git with gh and lazygit
- 03-tools.sh — ripgrep, fd, fzf, jq, yq, bat, eza, zoxide, sqlite, httpie, rclone,
  ffmpeg, imagemagick, poppler, tesseract, pandoc
- 04-inference.sh — llama.cpp built with CUDA for compute 6.1, whisper.cpp, piper,
  openWakeWord
- 05-services.sh — Open WebUI, SearXNG bound to loopback, OpenClaw gateway on Node
  (not Bun — the docs flag Bun as unstable for WhatsApp and Telegram sessions).
  All three are **socket-activated systemd user units, not enabled at boot** —
  they spawn on first connection and idle-stop after a timeout. XOS targets 8GB
  machines; three language runtimes resident at idle is unacceptable. Only xosd
  and llama-server start at boot. Exception: if messaging is configured, the
  OpenClaw gateway starts at boot, since inbound messages cannot socket-activate
  a listener that isn't listening.
- 06-opencode.sh
- 07-models.sh — hf CLI, pull the top-ranked model for detected hardware from
  verified-models.json
- 08-desktop.sh — Hyprland config, waybar module, Plymouth, theme
- 09-xosd.sh — build xosd, install the systemd user unit, enable it
- boot.sh — one-line entry point that runs Omarchy then install.sh

Also: xosd must install standalone on any existing Arch or Debian box via
09-xosd.sh alone. XOS is the turnkey version, not the only version.

Guardrail: never modify Omarchy's files in place — layer configuration on top so
upstream updates remain applicable. Pin every package version.

Check: on a clean Arch VM, boot.sh completes and reboots into a working XOS desktop
with xosd running and the status bar live.
```

### P13 amendment — installer must handle BIOS

Add to P13's prompt before running it:

```
Additional requirement: XOS ships its own base installer rather than using
Omarchy's ISO, because Omarchy's installer requires UEFI and does not support
legacy BIOS — which would exclude most pre-2012 machines, the core target.

The XOS installer must:
- Detect firmware mode and partition accordingly: GPT with an EFI system partition
  under UEFI, MBR with a BIOS boot partition under legacy
- Install GRUB for BIOS, systemd-boot or GRUB for UEFI
- Install base Arch, then invoke Omarchy's install script (which runs on any fresh
  Arch install), then apply the XOS layer
- Run install/00-hardware.sh before everything else
- Offer LUKS encryption as opt-in rather than mandatory — full-disk encryption on a
  decade-old CPU without AES-NI is painfully slow, and many target machines predate
  it. Detect AES-NI and recommend accordingly.

Keep the layering principle: Omarchy's own files are never modified in place.
```

## P14 — First-run experience

```
Build the XOS first-run wizard and the first goal.

Context: XOS is installed and xosd is running. This is the first 90 seconds a user
spends with the system, and it decides whether they keep it. Every step must be
skippable — someone booting with no accounts, no keys and no wifi must still reach
a working local assistant.

Wizard, in the TUI (default) with a matching Mission Control version:
1. Hardware summary from `xos hardware` — what was detected, which drivers loaded,
   which fallback level was reached, anything unavailable and why
2. Benchmark offer — run xos-bench against candidate models on this machine.
   Skippable; shows the ranked result if run.
3. Model — top-ranked pick with VRAM fit shown against detected hardware; download
   or skip
4. Provider keys — OpenRouter first, since it is one key rather than eight; then
   any others. Stored via Vault. Skippable.
5. Connectors — shells out to gh, rclone and OpenClaw's own credentials. XOS
   registers no OAuth apps of its own.
6. Messaging — WhatsApp QR link. Warn that a dedicated number is recommended and
   require an allowlist before enabling.
7. Voice — mic test, wake word selection
8. Policy — routing mode and firewall strictness, with plain-language descriptions
   of what each permits
9. Kill switch — show the keybinding and make the user press it once, so they have
   used it before they ever need it

Then run the first goal automatically. Do not drop the user at an empty prompt.

The first goal is built in and fixed: inventory this machine, read the obvious
project directories (~/Documents, ~/Projects, ~/code, ~/Downloads — read-only,
policy-gated), and write a summary of what the user appears to work on into
long-term memory.

It is chosen because it is zero-risk, demonstrates the complete loop — goal,
decomposition, node execution, memory write, Mission Control visibility — and
seeds memory so the second interaction is already personalised. The user watches
the system work end to end before trusting it with anything real.

Guardrail: the first goal is read-only. No writes outside memory, no network, no
irreversible actions. If any directory is missing or unreadable, skip it silently.
The wizard must complete and reach a usable state even if every step is skipped.

Check: complete the wizard skipping every optional step, confirm the first goal
runs and writes to long-term memory, confirm `xos chat` works offline immediately
afterwards.
```

