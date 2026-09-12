# The XOS Manual

XOS is a Linux desktop with an assistant built into it rather than bolted on
top. It runs a model on your own machine, remembers what you tell it, works on
goals that take longer than one conversation, and can act on your behalf — and
every one of those things is something you can watch, stop and undo.

It is built for the computer you already have. Not a new one.

---

## Before anything else: three things worth knowing

**It works with no internet.** The local model, your memory, the task graph and
every tool run on your machine. An API key makes XOS better at hard questions.
It is not required for XOS to work.

**One key combination stops everything.** `Ctrl+Alt+Escape`, from anywhere,
including when the desktop has stopped responding. Not a pause: every running
action stops, everything queued is dropped, and nothing starts again until you
say so. The first-run wizard makes you press it once, on purpose.

**Nothing it does is a surprise.** Anything that cannot be undone asks first.
Anything that can be undone is recorded with enough detail to reverse it. You
can see all of it, at any time, in one place.

---

## The state this is in

Read this part before you install it on anything you care about.

XOS is newly built and **has never been installed on real hardware**. Every
component is tested — several hundred automated tests — but "it boots into a
working desktop" is the one claim nobody has verified yet.

So: **use a virtual machine first.** It costs you twenty minutes and it is the
difference between finding out that a package pin is stale and finding out on
the machine you needed today.

There is an XOS install medium, and it is the intended way in. It has been
built and booted: on legacy BIOS and on UEFI, in a virtual machine, all the way
to a root shell where typing `install-xos` starts the installer. What has *not*
happened is a complete install onto a disk, and no physical machine has ever run
any of this.

---

## Installing

### What you need

- A 64-bit x86 machine. Genuinely any of them: XOS checks for baseline x86-64
  and nothing above it, because raising that floor would exclude the machines
  it exists for.
- A disk you are willing to erase completely.
- A USB stick, with the XOS medium written to it (or the official Arch ISO,
  which also works — see below).
- Ideally, an ethernet cable. Wifi during install works on most hardware and
  the installer tells you plainly when it does not.

A GPU is optional. With one, XOS runs a model locally. Without one, it still
runs — either slowly on the CPU, or through an API.

### With the XOS medium

Write the ISO to a USB stick, put it in the machine, and turn it on. **The
installer starts by itself.** It works out what hardware is here, shows you the
disks, asks which one, asks about encryption and a name, then partitions,
installs Arch, applies the XOS layer and offers to reboot.

Nothing is erased until you have seen the disk and typed its name back.

If you leave the installer, or it finishes, you get a shell. From there
`install-xos` starts it again, and `install-xos --dry-run` walks the whole thing
writing nothing — worth doing once before the real run.

#### Installing without being asked anything

The boot menu has a second entry: **Install XOS automatically (erases the
largest disk)**. It does exactly that — picks the largest disk in the machine,
erases it, and installs without another question.

It counts down for twenty seconds first, and `Ctrl+C` stops it. It is never the
entry you land on by pressing Enter, because a USB stick left in the wrong
machine should not cost somebody a disk.

### Building the medium

Needs an Arch machine, because `mkarchiso` does. This is the only part of XOS
with that requirement, and it exists so that nobody installing XOS has one.

```
git clone https://github.com/gigad1k/XOS
cd XOS
sudo ./iso/build.sh --check     # say what is missing, build nothing
sudo ./iso/build.sh             # build it
```

It puts the XOS source, prebuilt `xos` and `xosd` binaries, and the wifi drivers
that are not in the kernel onto the medium. The binaries matter: driver
resolution lives in the daemon, and without them the install would resolve no
drivers at all, on exactly the machines that most need it.

Then:

```
sudo dd if=out/xos-*.iso of=/dev/sdX bs=4M status=progress oflag=sync
```

`/dev/sdX` is the stick, not a partition on it, and everything on it goes.

### Without the XOS medium

The official Arch ISO works too. Boot it, then:

```
pacman -Sy git
curl -fsSL https://raw.githubusercontent.com/gigad1k/XOS/main/install/boot.sh | bash
```

`boot.sh` works out where it is. On install media it stops and shows you what to
run, because the next step erases a disk and that should be typed by somebody who
has read the sentence saying so.

```
# see the plan, change nothing:
/tmp/xos-install/install/base.sh --dry-run --disk /dev/sdX

# do it. It names the disk and asks you to type the name back:
/tmp/xos-install/install/base.sh --disk /dev/sdX

# then the XOS layer:
/tmp/xos-install/install/install.sh
```

Then reboot.

This route has no prebuilt binaries on it, so driver resolution during the
install is conservative: it installs what is safe on any machine rather than what
the database says your card needs. The XOS medium is better for that reason.

### The one file that destroys data

Whichever route you take, `base.sh` is the only file in XOS that destroys data. It shows you the disk and
everything on it, states that all of it will be gone, and refuses to run at all
if nothing is attached to the prompt to say no.

### Encryption

Off by default, and available with `--encrypt`.

This is deliberate rather than lazy. On a CPU without AES-NI — which many of the
machines XOS targets are — full-disk encryption is done in software and makes an
already slow machine noticeably slower at everything that touches the disk. XOS
detects AES-NI and tells you which case you are in before you choose. With
AES-NI it costs almost nothing and you should probably turn it on.

### UEFI and legacy BIOS

Both work. XOS ships its own base installer specifically because Omarchy's
requires UEFI, and most machines from before about 2012 boot legacy BIOS — which
is not an edge case for XOS, it is the target.

If XOS cannot tell which firmware you have, it stops rather than guessing. A
guess produces a machine that installs perfectly and then does not boot.

### With no desktop at all

XOS runs headless: the daemon, a local model, and nothing that draws a window.
For a box in a cupboard that answers over SSH, see
[the headless manual](headless.md).

### Just the daemon, on a machine you already have

XOS is the turnkey version, not the only version. To put the daemon on an
existing Arch or Debian box and nothing else:

```
git clone https://github.com/gigad1k/XOS
cd XOS && ./install/09-xosd.sh
```

That script stands alone. It touches no desktop, no bootloader and nothing else
on the machine.

---

## The first ninety seconds

The first time you log in, the wizard runs. Nine steps:

| | Step | What it is |
|---|---|---|
| 1 | What you have | Everything detected, which drivers loaded, and what is **not** working and why |
| 2 | How fast it is | Offers to measure the models on your actual machine |
| 3 | The model that runs here | The recommendation, with what it needs beside what you have |
| 4 | Provider keys | OpenRouter first, because it is one key rather than eight |
| 5 | Connectors | GitHub, cloud storage, messaging — each through its own login |
| 6 | Messaging | WhatsApp, with the warnings before the offer |
| 7 | Voice | Microphone test and wake word |
| 8 | What XOS is allowed to do | Routing and how often it asks, in plain words |
| 9 | The stop button | Press `Ctrl+Alt+Escape` once, so you have used it |

**Every step can be skipped, and XOS works afterwards either way.** That is not
a disclaimer, it is the design: somebody booting with no accounts, no keys and
no wifi must still end up with a working assistant. Skip all nine and XOS tells
you so, and still works, offline.

Run it again whenever you like:

```
xos setup
```

### Then it does one thing by itself

The wizard does not leave you at an empty prompt. It runs one goal while you
watch: take stock of the machine, look at `~/Documents`, `~/Projects`, `~/code`
and `~/Downloads`, and write down what you appear to work on.

It is chosen because it cannot go wrong. It **lists** those directories and
opens nothing. It writes one entry to memory and nothing else. Nothing leaves
the machine. A directory that is missing or private is skipped without comment.

You watch a goal get broken into steps, the steps run, and memory get written —
the whole architecture, once, before you are asked to trust it with anything
real.

---

## Talking to it

```
xos chat
```

That is the main way to use XOS. It is a terminal interface because it starts
instantly and works on hardware where a browser struggles.

- `Enter` sends
- `Ctrl+L` clears
- `Ctrl+C` leaves

The bar at the bottom shows which tier answered — **local** or **api** — and
what it is doing. Agent work is genuinely slow: every tool call is a full round
trip to a model. XOS shows you progress per step rather than a spinner, so two
minutes of real work reads as working rather than hung.

Open WebUI is available as well, if you prefer a browser. It is opt-in and
socket-activated, so it uses no memory until you open it.

---

## Goals

A goal is something too big for one answer. You declare it; XOS breaks it into
steps and works through them.

```
xos goal new "sort out my photo backups"
xos goal list
xos goal show goal-1c8ff3a8
xos goal advance
```

Steps that need your approval stop and wait. They do not retry forever and they
do not decide for you. Approve them from the chat, from Mission Control, or by
letting the next `advance` pick them up after you have said yes.

---

## Memory

Three tiers:

- **Working** — the current task. Minutes to hours.
- **Session** — today. The conversation, recent files, today's decisions.
- **Long-term** — persistent. Preferences, projects, people, how you work.

```
xos memory search "photo backup"
xos memory stats
xos memory write --tier long-term "I prefer British spelling"
xos memory promote --from working --to session
```

Memory never leaves your machine. Anything bound for an API goes through a
digest builder that summarises and redacts first; raw memory is never
transmitted.

**One honest gap.** Memory is written and searchable, but XOS does not yet pull
it into a conversation by itself. Ask it something it "knows" and it will tell
you it does not know, because the answering path does not consult memory. `xos
memory search` finds it perfectly. This is the single most valuable thing left
to build, and it is written down as such in the project's own notes.

---

## Backing it up

The whole value of XOS is that it remembers you, so a dead disk must not erase
that.

```
xos export ~/xos-backup.age
xos import ~/xos-backup.age
```

The bundle is encrypted and holds memory, the task graph, compiled prompts and
configuration. It records **which** providers you had configured, never their
keys. Import merges rather than overwrites: nothing already there is lost.

---

## What XOS is allowed to do

Two independent settings.

### How much it spends

```
xos mode                  # what is it now
xos mode aggressive-local # slower, free
xos mode balanced         # local first, escalating when it helps
xos mode best-quality     # the strongest model available, and the most expensive
```

Every time XOS escalates from the local model to an API, it is logged with the
reason:

```
xos escalations
xos spend --days 30
```

### How often it asks

- **strict** — asks before anything that writes, not only before what cannot be
  undone.
- **standard** — asks before anything that cannot be undone. The default.
- **permissive** — asks rarely.

Permissive relaxes the *prompting* only. Designated-secret paths stay blocked
and API-bound context stays redacted at every setting. Those are not
preferences.

Set it in the wizard, or in `~/.config/xos/config.toml` under `[policy]`. It
takes effect when the daemon next starts.

```
xos policy log                        # what it decided, and why
xos policy test delete_file '{"path": "/etc/fstab"}'   # what it would decide
```

`policy test` is worth knowing about. It answers the question without doing the
thing.

---

## Stopping it

```
Ctrl+Alt+Escape     # from anywhere
xos halt            # the same thing
xos resume          # start again
```

A halt survives a restart. If you halt XOS and reboot, it is still halted. It
stays stopped until you say otherwise, which is the only behaviour that makes a
stop button worth having.

---

## Undoing it

Every action XOS takes on your files is recorded with enough state to reverse
it.

```
xos journal              # what it has done, and what can still be undone
xos journal --limit 50
xos undo                 # reverse the last one
xos undo --last 5        # reverse the last five
```

Undo is transactional: it checks that it still can before it starts, and either
reverses the lot or none of it. A file that changed since XOS touched it is not
quietly overwritten.

---

## Mission Control

```
http://127.0.0.1:7777
```

Everything at once: the task graph as it runs, what needs you, spending beside
electricity, recent escalations, policy decisions, memory, the heartbeat.

Loopback only. It is a window onto your machine and has no business being
reachable from anywhere else.

It can do two things besides show you: approve a blocked step, and change the
cost mode. Plus halt, which is always within reach. Everything else is a
window.

---

## The heartbeat

XOS wakes periodically rather than running continuously.

```
xos pulse status    # what it is doing, and what it costs to do it
xos pulse tasks     # scheduled work and watchers
```

`pulse status` puts today's API spend next to today's electricity, in the same
units, because on an old machine with a hungry GPU the second number is
sometimes larger than the first and nobody ever tells you that.

---

## Hardware and drivers

```
xos hardware                 # what is in this machine
xos hardware --json          # the same, for scripts
xos hardware --device 10de:1b80   # what would XOS do with a card you do not own
```

XOS keeps a community table mapping PCI IDs to drivers, with a fallback chain
for each. The governing rule is that **every path ends at something that shows a
picture**. A degraded desktop is recoverable by the person sitting at it; a
black screen is not.

- **NVIDIA** resolves by device ID to the branch that still supports the card —
  580 for Maxwell, Pascal and Volta, 470 for Kepler, 390 for Fermi, current for
  Turing and newer. The branch is **pinned** so an update cannot replace it with
  one that does not drive your card.
- **AMD** and **Intel** use the in-tree drivers. Old GCN 1.0 and 1.1 cards get
  the kernel parameters that stop them falling back to `radeon` silently.
- **Virtual machines** — virtio, QEMU, VMware, VirtualBox, Hyper-V — are all
  covered, which matters because that is where you will try XOS first.
- **Old server and workstation chips** — Matrox, ASPEED, VIA, SiS, Cirrus, S3 —
  are covered too.
- **Anything else** is driven generically: the generic userspace over whatever
  driver the kernel already bound, then `modesetting`, then a plain framebuffer.

XOS will never install a driver for a card it does not recognise. Naming a
plausible driver that turns out to be wrong costs you a reboot and gives you a
wrong theory about why your screen is black. A generic fallback that works is
better than a specific guess that might not.

If you get a card working that XOS did not know about, `xos hardware --submit`
offers your machine's details to the community database. It shows you exactly
what would be sent, in full, and sends nothing without a yes. No hostname, no
user name, no serial number, no network address — hardware identifiers only.

---

## Models

XOS picks a local model that your machine can actually hold, from a catalogue
that says which machines each one suits.

- A capable GPU gets the best model that fits alongside your desktop.
- A smaller card or no card gets something smaller that genuinely runs.
- A machine that can run neither is told so, and works through an API instead of
  downloading four gigabytes it cannot use.

The thresholds are written against what machines **report**, not what they were
sold as. A machine sold as 8GB reports about 7800MB, because firmware takes its
share first. Every round number in that catalogue would have excluded the exact
machine it was written for.

---

## Where things live

| What | Where |
|---|---|
| Configuration | `~/.config/xos/config.toml` |
| Memory and the task graph | `~/.local/share/xos/` |
| Credentials | the system keyring, encrypted |
| Install log | `/var/log/xos-install.log` |
| Hardware report | `/var/log/xos-hardware.log` |
| Mission Control | `http://127.0.0.1:7777` |

---

## What leaves your machine

Nothing, unless you configured an API and asked a question that needed it.

When something does go to an API:

- It goes through a digest builder that summarises and redacts first. Raw memory
  is never transmitted.
- Designated-secret paths are blocked outright, at every strictness setting.
- The escalation is logged with its trigger, and you can read it with `xos
  escalations`.

The community hardware submission is opt-in, never automatic, and shows you the
entire contents before asking.

---

## When something goes wrong

**The screen is black after installing.** The fallback chain exists for this.
Boot with `nomodeset` on the kernel command line to reach a framebuffer, then
read `/var/log/xos-hardware.log` — it lists every device, the driver chosen, and
which fallback level it reached.

**No wifi after installing.** The install log says so, unmissably, with what to
try. XOS works offline in the meantime, on the local model. Use ethernet or USB
tethering from a phone to get going.

**`xos` says it cannot reach the daemon.** `systemctl --user status xosd`. It
runs as a user service, with lingering enabled so it survives you logging out.

**It feels slow.** It probably is. Every tool call is a round trip to a model,
and on an old GPU that takes what it takes. `xos pulse status` shows what is
loaded and what it is doing. If nothing is loaded, the first request pays to
load the model.

**It did something you did not want.** `xos journal`, then `xos undo`. If it is
still going: `Ctrl+Alt+Escape`.

**A package would not install.** Nothing in the XOS install can abort the
install; every failure is logged and skipped. Read `/var/log/xos-install.log`,
install the missing thing yourself, and rerun that step:

```
bash install/install.sh --only 07-models.sh
```

---

## Helping

Two community tables ship with XOS, and both take pull requests.

- `hardware-db.json` — PCI IDs to drivers. Add a row if you get a card working
  that XOS did not know about.
- `verified-models.json` — which model suits which machine. Add a row if you run
  one successfully, and say what you measured.

Every row carries a confidence. `confirmed` means somebody ran it on that
hardware. `known` means it should work and nobody has proved it. Say which yours
is, and say what machine you tested on.

---

## Known limits, stated plainly

- **Never installed on real hardware.** The medium has been booted on both
  firmwares in a virtual machine and the installer has been run as far as
  choosing a disk. Nothing has been installed onto a disk and rebooted, and no
  physical machine has run any of it. Use a VM first.
- **Memory is not recalled into conversations.** It is written and searchable;
  the answering path does not consult it yet.
- **Package versions are pinned to what was current when written.** Arch moves. A
  stale pin degrades the install rather than breaking it — XOS reports what it
  could not get and carries on.
- **One `confirmed` graphics row.** The GTX 1080. Everything else in the NVIDIA
  table is from documentation, not from somebody's machine.
- **The install medium has never been built or booted.** The archiso profile is
  written and its own tests pass, but `mkarchiso` needs an Arch machine and there
  has not been one. Both firmware paths, the boot menus and the bundled drivers
  are unproven in the only way that counts.
- **The bundled wifi drivers are built from the AUR at image-build time.** If the
  AUR is unreachable or a package does not build, the medium is made without
  them and says so, and the deepest wifi fallback then has nothing to try.
