# XOS — Project Context

## What this is
An AI-native Linux OS. A Rust daemon (`xosd`) is the intelligence layer; everything
else plugs into it. Layered over Omarchy (Arch + Hyprland), never forked.

## Thesis
A trustworthy local AI that lives on the desktop, understands system state, remembers
context, manages long-running goals, and takes action without constant prompting.

## Architecture rule — never violate
Nothing imports a model provider, MCP server or channel gateway directly.
Everything goes through `xosd`. UIs are thin clients over `xosd`'s API.

## Component naming (public / internal)
- XOS Core / `xosd` — the daemon
- XOS Goals / `graph` — long-running goal decomposition and execution
- XOS Memory / `memory` — three-tier persistent memory
- XOS Pulse / `scheduler` — heartbeat, cron, watchers
- XOS Mission Control — transparency UI
- XOS Vault / `vault` — credentials in system keyring
- XOS Connect / `providers` — connectors
- XOS Bench / `bench` — model validation

## Two-tier cognition
- **Gemma 4 E4B (local)** — reflex layer. Always available. Executes graph nodes,
  handles chat, runs tools. Confirmed working on GTX 1080.
- **Cloud API (supervisor)** — wakes on events only, never continuous. Decomposes
  goals, re-plans failures, compiles prompts for the local model, reviews batches.

## Hardware target
Ryzen + iGPU, 32GB RAM, GTX 1080 8GB (Pascal, compute 6.1, NVIDIA 580 branch pinned).
iGPU drives display; the 1080 is a headless CUDA device.
Avoid FP16 flash-attention paths — GP104 FP16 is 1/64 rate. Quantised kernels only.

## Non-negotiables
1. Works offline. Local model + tools must function with no network.
2. No decorative colour. See STYLE.md — three semantic hues only.
3. Secrets never leave the machine. Designated-secret paths blocked; all
   API-bound context egress-scanned and redacted.
4. Irreversible actions prompt the user. Reversible ones don't.
5. Every local→API escalation is logged with its trigger.
6. Idle resident memory under 1.2GB excluding the model. Nothing runs at boot
   that isn't needed at boot — services are socket-activated and spawn on first
   use. XOS targets machines with 8GB total.
7. `xos chat` (TUI) is the default interface. Open WebUI is opt-in. The TUI is
   instant, has no licence constraints, and works on hardware where Chromium
   struggles.
8. There is always a kill switch. One keystroke halts every autonomous action
   from anywhere in the system. It is never more than one step away and it
   always works, including when the UI is unresponsive.
9. Every action is reversible or journalled. Irreversible actions prompt;
   reversible ones are recorded with enough state to undo them.

## Latency principles
- Compiled prompts are stable per task type, so their KV cache stays warm in
  llama.cpp and is never reprocessed. This is the single largest latency win
  available — treat cache reuse as a requirement, not an optimisation.
- Agent work is inherently slow: every tool call is a full model round trip. Do
  not hide this. Show node-level progress so a two-minute graph reads as working
  rather than hung.

## Stack
Rust (daemon, CLI) · SQLite + sqlite-vec (memory, graph) · llama.cpp server (local
inference) · system keyring (secrets) · ratatui (TUI) · IBM Plex Sans/Mono.

## Style
See STYLE.md. Instrumentation, not consumer app. Density over airiness. 3px radius.
No shadows. Motion only where it conveys state change.
