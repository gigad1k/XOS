<p align="center">
  <img src="brand/xos-logo.svg" alt="XOS" width="130">
</p>

# Working on XOS

## Build it

The workspace is Rust. Everything builds on Linux; the daemon reads `/proc` and
`/sys`, so it does not build meaningfully on Windows or macOS — use WSL or a VM.

```
cargo build --release
cargo test --workspace
```

## Run the tests

There are two kinds and both matter.

```
cargo test --workspace            # the daemon, the CLI, the bench harness
bash install/tests/hardware.test.sh
bash install/tests/install.test.sh
bash iso/tests/iso.test.sh
bash docs/manual.test.sh          # every command the manuals name must exist
```

The shell suites never install anything. They run each step against a fake root
with a stubbed package manager, because a test suite that can install packages
on the machine running it is a test suite that eventually does.

## The rule that has been broken most

**A step runs on the machine doing the building. It must act on the machine
being built.**

That mistake has been made eight times in this repository, in five files, and
every instance shipped something broken: a driver installed into a live medium's
RAM, a desktop installed into the same, a boot parameter written after the boot
config was generated, and five steps that skipped themselves and said something
reasonable while doing it.

So:

- Install packages with `pin_install`, never `pacman` directly.
- Run anything else through `in_target` from `install/lib.sh`.
- Never ask `command -v` about a tool an earlier step installed into the target.
- If you write to a file something else reads, check what order they run in.

`install/tests/install.test.sh` greps for all four. If you add a step, it will
grep yours too.

## Adding hardware

`hardware-db.json` is the table. A row needs a confidence and it has to be true:

| | Means |
|---|---|
| `confirmed` | you ran it on that hardware |
| `known` | it should work and nobody has proved it |
| `inferred` | XOS reasoned from the vendor because the table had no row |
| `generic` | no vendor row; the path that produces a picture on anything |

Never write `confirmed` for something you have not run. The installer shows that
word to people deciding whether to trust a driver.

Check what XOS would do before and after your change:

```
xos hardware --device 10de:2684
```

## Style

`STYLE.md` is not a suggestion. The one that catches people out: **there are
three colours and each means exactly one thing.** Nothing else is ever coloured,
including a logo. Emphasis is weight and tone.

## Commits

Say what was wrong and what the consequence was, not what the diff does. A
message that says "fix driver installation" is worth less than one that says the
driver went into a filesystem the reboot discards, which is why the screen was
black.

Every fix that could regress gets a test in the same commit.

## What is not done

`BUILD.md` is the ledger, and the last section lists what has not been proven.
The headline: no physical machine has run any of this. If you have one, that is
the single most useful thing you could contribute.
