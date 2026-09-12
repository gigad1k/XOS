# XOS Headless

Running XOS with no desktop, no monitor and nobody sitting at it.

This is XOS as a daemon on a box in a cupboard: a machine that holds a local
model, remembers things, works on goals, and answers over SSH or a messaging
account. Everything in [the manual](manual.md) still applies — this covers what
is different when there is no screen.

---

## Why you would

The desktop is the turnkey version. It is not the only one, and for a lot of
machines it is the wrong one.

An old tower with a decent GPU makes a much better always-on assistant than it
does a desktop. It sits somewhere out of the way, costs a few watts, and gives
you a local model on your own network that nothing leaves. XOS was built so
that the daemon is the product and every interface is a thin client over it —
which means the interfaces are optional.

Three shapes are worth knowing about:

| Shape | What it is |
|---|---|
| **A box on your network** | The daemon and a local model. You reach it over SSH from wherever you are. |
| **The model, shared** | The same, plus `llama-server` on the network, so other machines borrow the GPU. |
| **Answering messages** | The daemon plus the OpenClaw gateway, replying on WhatsApp to numbers you name. |

---

## Installing without a desktop

### On a machine you already have

The daemon installs standalone on any existing Arch or Debian box. This is the
quickest way in and it touches nothing else:

```
git clone https://github.com/gigad1k/XOS
cd XOS && ./install/09-xosd.sh
```

That script builds `xosd` and `xos`, installs the hardware and model databases
beside them, writes a user service and enables lingering so the daemon survives
you logging out. It installs no desktop, no bootloader and no display server.

Then:

```
systemctl --user start xosd
xos status
```

### From the install medium

Boot the XOS medium and let the installer run, but stop before the desktop:

```
install-xos --dry-run     # see the plan first
install-xos
```

then, once it has finished the base system, apply only the parts you want:

```
bash /root/xos/install/install.sh --only 09-xosd.sh    # the daemon alone
bash /root/xos/install/install.sh --only 04-inference.sh  # and a local model
```

`--only` runs a single step. The desktop is `08-desktop.sh`; skip it and
nothing on the machine draws a window.

---

## Reaching it

### Over SSH

The daemon listens on a Unix socket, not a network port, which is deliberate:
nothing about XOS is reachable from the network unless you arrange it.

```
ssh you@the-box
xos chat
```

`xos chat` is a terminal program, so it works over SSH exactly as it does
locally. Everything else does too:

```
ssh you@the-box xos status
ssh you@the-box xos hardware
ssh you@the-box 'xos goal new "tidy the downloads folder"'
ssh you@the-box xos pulse status
```

### Mission Control, without exposing it

Mission Control binds to loopback on the machine it runs on, and should stay
there. Forward it over SSH rather than opening a port:

```
ssh -N -L 7777:127.0.0.1:7777 you@the-box
```

Then open `http://127.0.0.1:7777` on your own machine. The connection is
SSH's; nothing new is listening on the network.

### Sharing the model with other machines

`llama-server` binds to loopback too. If you want other machines on your
network to use the GPU, forward it the same way rather than rebinding it:

```
ssh -N -L 8080:127.0.0.1:8080 you@the-box
```

Point anything that speaks the OpenAI API at `http://127.0.0.1:8080/v1`. If you
would rather bind it to the network properly, change `--host` in
`~/.config/xos/llama.env` — but understand that you are then running an
unauthenticated model server, and put it behind something.

---

## What runs, and what it costs

A headless XOS is two resident processes: `xosd` and `llama-server`. Everything
else is socket-activated and exists only between the first connection and
fifteen idle minutes later.

```
xos pulse status
```

That reports what is loaded, how long it has been idle, today's API spend and
today's electricity side by side. On a box that is always on, the second number
is the one that matters, and it is the one nobody usually tells you.

To keep the model out of memory until something asks for it, set the GPU layers
to zero in `~/.config/xos/llama.env` and let it load on demand — slower first
answer, nothing resident.

---

## The first run, with no screen

The wizard works over SSH like anything else:

```
xos setup
```

Every step is skippable and the machine works afterwards either way. On a
headless box two steps are worth doing:

- **Provider keys**, if you want the API tier at all. Without one the machine
  runs entirely on its local model, which is a perfectly good way to run it.
- **What XOS is allowed to do.** On a machine nobody is watching, `strict` is
  the sensible setting: it asks before anything that writes, not only before
  what cannot be undone. Anything it asks about waits in the task graph.

The kill switch step asks you to press `Ctrl+Alt+Escape`, which is a desktop
keybinding and does nothing over SSH. The command is the same thing:

```
xos halt      # stops everything, and stays stopped across a reboot
xos resume
```

Learn that one. It is the whole of the safety story on a machine you are not
sitting at.

---

## Work that waits for you

A headless box will reach things it is not allowed to do on its own. Those stop
and wait rather than guessing:

```
xos goal list                 # what it is working on
xos goal show goal-1c8ff3a8   # the steps, and what they produced
xos journal                   # what it has actually done
xos undo --last 1             # reverse it
```

A step that needs you sits in `needs-user` until you approve it — from the
chat, from Mission Control over the SSH tunnel, or by approving and letting the
next `xos goal advance` pick it up.

Nothing retries forever, and nothing decides on your behalf because you were
not there to ask.

---

## Messaging, which is the one exception

Everything in XOS is socket-activated except the daemon, the model, and — if
you configure messaging — the OpenClaw gateway. That one starts at boot,
because an inbound message arrives at a listener or it does not arrive at all,
and a socket cannot activate a listener for a message nobody sent yet.

On a headless box this is the most useful interface there is: you message the
machine and it answers. Two things before you turn it on:

- **Use a dedicated number.** Linking your own account means XOS sees every
  conversation on it.
- **The allowlist is required, not advisory.** XOS replies only to numbers you
  name. An assistant that answers anyone who messages it is a different and
  much worse thing.

```
xos setup     # step 6
```

---

## Keeping it

The value of the machine is what it remembers, and a headless box is exactly
the kind that quietly dies without anyone noticing until they need it.

```
xos export /somewhere/else/xos-backup.age
```

The bundle is encrypted and holds memory, the task graph, compiled prompts and
configuration. It records which providers were configured, never their keys.
Put it somewhere that is not that machine, on a schedule.

Restoring merges rather than overwrites:

```
xos import /somewhere/else/xos-backup.age
```

---

## When you cannot see it

```
systemctl --user status xosd          # is it up
journalctl --user -u xosd -n 50       # what it said
xos status                            # health, halt flag, providers
xos hardware                          # what the machine thinks it is
xos policy log                        # what it decided, and why
xos escalations                       # every time it reached for an API
```

If `xos` cannot reach the daemon, the socket is the thing to check. It lives
under `$XDG_RUNTIME_DIR`, which only exists while you have a session — that is
what lingering is for, and `09-xosd.sh` enables it. If the daemon dies when you
log out, lingering is off:

```
loginctl enable-linger $USER
```

---

## What this does not give you

- **No voice.** Wake words and the microphone belong to a machine somebody is
  near.
- **No status bar.** `xos pulse status` is the same information, on demand.
- **No Open WebUI unless you tunnel it.** It is socket-activated on 8081 and
  bound to loopback, same as Mission Control.
- **Hardware detection still works**, and is worth running once: `xos hardware`
  tells you whether the box can hold a local model at all, and what it would
  need to.
