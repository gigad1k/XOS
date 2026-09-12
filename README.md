# XOS

An AI-native Linux OS. A Rust daemon (`xosd`) is the intelligence layer;
everything else plugs into it. Layered over Omarchy (Arch + Hyprland), never
forked. Nothing imports a model provider, MCP server or channel gateway
directly — everything goes through `xosd`, and every UI is a thin client over
its API.

XOS aims to be a trustworthy local AI that lives on the desktop, understands
system state, remembers context, manages long-running goals, and takes action
without constant prompting. It is built to three rules: it works offline, every
escalation to the cloud is logged with its trigger, and one keystroke halts
every autonomous action.

Status: scaffold. The crates are laid out and the subsystem modules are empty.
Nothing is implemented yet — `BUILD.md` tracks what lands next.

## Hardware compatibility

| Hardware | Local model | Status |
|---|---|---|
| Ryzen + GTX 1080 8GB | Gemma 4 E4B | confirmed working |

Report a machine by opening a pull request that adds a row.

## Workspace

| Crate | Binary | Responsibility |
|---|---|---|
| `xosd` | `xosd` | XOS Core — the daemon and every subsystem |
| `xos-cli` | `xos` | Command line client and chat TUI |
| `xos-bench` | `xos-bench` | XOS Bench — model validation |

## Build

```
cargo build
cargo run --bin xos
```

## Documentation

| File | Contents |
|---|---|
| `CLAUDE.md` | Architecture context and non-negotiables |
| `STYLE.md` | Visual language and copy voice |
| `BUILD.md` | Build ledger, task status and gates |
| `PROMPTS.md` | The build prompts, P0 through P14 |
| `docs/spec.md` | The full specification |

## Licence

MIT. See `LICENSE`.

## Installing

Write the XOS medium to a USB stick, boot it, and type `install-xos`. That is the
whole installation: it works out the hardware, asks which disk, and does the rest.

Building the medium needs an Arch machine, because `mkarchiso` does:

```
sudo ./iso/build.sh --check     # say what is missing, build nothing
sudo ./iso/build.sh             # build it into out/
```

Neither the image nor the install has been run on real hardware yet. Use a
virtual machine first.

## The manual

[docs/manual.md](docs/manual.md) — installing, running, supervising and stopping XOS.

[docs/headless.md](docs/headless.md) — running it with no desktop: a box on your
network with a local model, reached over SSH.

Every command in both is checked against the binary by `docs/manual.test.sh`.
