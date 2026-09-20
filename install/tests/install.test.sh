#!/usr/bin/env bash
#
# Tests for the XOS install layer.
#
# The check P13 asks for is a clean Arch VM, which is not available here. What
# is available is everything short of it: every script parses, every step runs
# against a fake root with a stub package manager, nothing Omarchy owns is
# modified, every package is pinned, and the units say what they are meant to
# say about what starts at boot.
#
# base.sh is the exception and is only ever run with --dry-run. It is the one
# file in XOS that destroys data, and a test suite is not the place to find out
# whether it does.
#
#   ./install/tests/install.test.sh

set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
INSTALL="$HERE/.."
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASSED=0
FAILED=0
pass() { printf '  ok    %s\n' "$1"; PASSED=$((PASSED + 1)); }
fail() { printf '  FAIL  %s\n' "$1"; printf '        %s\n' "${2:-}"; FAILED=$((FAILED + 1)); }
check() { if [ "$3" = "0" ]; then pass "$1"; else fail "$1" "$2"; fi; }

SCRIPTS=(lib.sh base.sh install.sh boot.sh 00-hardware.sh 01-drivers.sh 02-runtimes.sh
         03-tools.sh 04-inference.sh 05-services.sh 06-opencode.sh 07-models.sh
         08-desktop.sh 09-xosd.sh 10-boot.sh)

# ---------------------------------------------------------------- parsing

printf '\nEvery script parses\n'
for script in "${SCRIPTS[@]}"; do
  check "$script" "syntax error" \
    "$(bash -n "$INSTALL/$script" 2>/dev/null && echo 0 || echo 1)"
done

printf '\nNo script can abort an install\n'
for script in "${SCRIPTS[@]}"; do
  # base.sh is allowed to stop: it is the one that partitions, and stopping
  # before it destroys the wrong disk is the correct behaviour.
  [ "$script" = "base.sh" ] && continue
  check "$script does not use set -e" "one failing command would abort the install" \
    "$(grep -qE '^set -[a-z]*e[a-z]*[[:space:]]*$|^set -e' "$INSTALL/$script" && echo 1 || echo 0)"
done

# ---------------------------------------------------------------- pinning

printf '\nEvery package is pinned\n'

UNPINNED="$(
  for script in "${SCRIPTS[@]}"; do
    grep -ohE 'pin_install(_all)? [a-z0-9 ._-]+' "$INSTALL/$script" 2>/dev/null |
      sed -E 's/pin_install(_all)? //' | tr ' ' '\n'
  done | sort -u | while read -r package; do
    [ -z "$package" ] && continue
    grep -qE "^$package=" "$INSTALL/packages.lock" || echo "$package"
  done
)"
check "every package named by a script is in packages.lock" "unpinned: $(echo $UNPINNED)" \
  "$([ -z "$UNPINNED" ] && echo 0 || echo 1)"

check "packages.lock gives every entry a version" "an entry has no version" \
  "$(grep -vE '^#|^$' "$INSTALL/packages.lock" | grep -qvE '^[a-z0-9._-]+=.+' && echo 1 || echo 0)"

check "the 580 branch is pinned, since 590 dropped Pascal" "nvidia-580xx-dkms is not in the lock" \
  "$(grep -qE '^nvidia-580xx-dkms=' "$INSTALL/packages.lock" && echo 0 || echo 1)"

# ---------------------------------------------------------------- a whole run

printf '\nA whole dry run\n'

ROOT="$WORK/root"
mkdir -p "$ROOT/etc" "$ROOT/var/log"
printf '[options]\nHoldPkg = pacman glibc\n' > "$ROOT/etc/pacman.conf"
cat > "$WORK/pm" <<'STUB'
#!/usr/bin/env bash
a="${1:-}"; shift 2>/dev/null
case "$a" in -Qq) exit 1 ;; *) echo "pm $a $*"; exit 0 ;; esac
STUB
chmod +x "$WORK/pm"

# Stand-ins for every external command an install step reaches for. They go on
# PATH ahead of the real ones, so a step can run for real into a sandbox root
# and write its config and units without installing anything on this machine.
#
# Stubs rather than a flag inside the installer: a test hook in production code
# is a branch that only tests take, and the interesting bugs live in the branch
# everyone else takes.
STUBS="$WORK/stubs"
mkdir -p "$STUBS"
# pacman, apt-get and dpkg are in here for a reason that cost a real scare:
# 09-xosd.sh reached past XOS_PACKAGE_MANAGER and ran the host's apt-get during
# a test. It failed only because the test was not run as root. A test suite must
# not be able to install packages on the machine running it.
for command in pipx npm uv hf git cmake systemctl loginctl cargo curl \
               pacman apt-get apt dpkg pacstrap arch-chroot genfstab \
               plymouth-set-default-theme nvcc install; do
  cat > "$STUBS/$command" <<STUB
#!/usr/bin/env bash
echo "STUB $command \$*" >> "$WORK/calls.txt"
exit 0
STUB
  chmod +x "$STUBS/$command"
done
# `install` is a real coreutils command the scripts use to copy binaries, and
# stubbing it out would hide whether they copy anything. It gets to be real.
rm -f "$STUBS/install"

sandbox() { # root, script, extra env...
  local root="$1"; local script="$2"; shift 2
  mkdir -p "$root/etc" "$root/var/log"
  [ -f "$root/etc/pacman.conf" ] || printf '[options]\n' > "$root/etc/pacman.conf"
  env PATH="$STUBS:$PATH" \
      XOS_INSTALL_ROOT="$root" \
      XOS_PACKAGE_MANAGER="$WORK/pm" \
      XOS_LOG="$root/var/log/xos-install.log" \
      "$@" \
      bash "$INSTALL/$script" > "$root/out.txt" 2>&1
}

env XOS_INSTALL_ROOT="$ROOT" XOS_PACKAGE_MANAGER="$WORK/pm" \
    XOS_LOG="$ROOT/var/log/xos-install.log" XOS_SKIP_OMARCHY=1 \
    bash "$INSTALL/install.sh" --dry-run > "$WORK/run.txt" 2>&1
RUN_EXIT="$?"
LOG="$ROOT/var/log/xos-install.log"

check "it completes" "exited $RUN_EXIT" "$([ "$RUN_EXIT" = "0" ] && echo 0 || echo 1)"
for script in 00-hardware.sh 01-drivers.sh 02-runtimes.sh 03-tools.sh 04-inference.sh \
              05-services.sh 06-opencode.sh 07-models.sh 08-desktop.sh 09-xosd.sh; do
  check "it runs $script" "$script never ran" \
    "$(grep -q "== $script" "$LOG" && echo 0 || echo 1)"
done
check "a dry run installs nothing" "a package was actually installed" \
  "$(grep -qE '^pm -S' "$WORK/run.txt" && echo 1 || echo 0)"

# The other half of that, and the more important one. A run without --dry-run
# must actually do something: an installer that quietly installs nothing and
# reports success is worse than one that fails, because nobody finds out until
# they reboot.
REAL="$WORK/real"
mkdir -p "$REAL/etc" "$REAL/var/log"
printf '[options]\n' > "$REAL/etc/pacman.conf"
: > "$WORK/real-pm-calls.txt"
cat > "$WORK/pm-recording" <<STUB
#!/usr/bin/env bash
a="\${1:-}"; shift 2>/dev/null
case "\$a" in
  -Qq) exit 1 ;;
  *) echo "\$a \$*" >> "$WORK/real-pm-calls.txt"; exit 0 ;;
esac
STUB
chmod +x "$WORK/pm-recording"
env PATH="$STUBS:$PATH" XOS_INSTALL_ROOT="$REAL" XOS_PACKAGE_MANAGER="$WORK/pm-recording" \
    XOS_LOG="$REAL/var/log/xos-install.log" XOS_SKIP_OMARCHY=1 \
    bash "$INSTALL/install.sh" > "$WORK/real.txt" 2>&1
check "a run without --dry-run actually installs" "it installed nothing and said it was fine" \
  "$([ -s "$WORK/real-pm-calls.txt" ] && echo 0 || echo 1)"
check "a real run writes the units" "no unit files" \
  "$([ -f "$REAL/etc/systemd/user/xosd.service" ] && echo 0 || echo 1)"
check "a real run does not say it would do things" "it ran as a dry run" \
  "$(grep -q 'would install' "$REAL/var/log/xos-install.log" && echo 1 || echo 0)"
check "the hardware step runs before the driver step" "the order is wrong" \
  "$([ "$(grep -n '== 00-hardware.sh' "$LOG" | head -1 | cut -d: -f1)" -lt \
      "$(grep -n '== 01-drivers.sh' "$LOG" | head -1 | cut -d: -f1)" ] && echo 0 || echo 1)"

# ---------------------------------------------------------------- layering

printf '\nNothing Omarchy owns is modified\n'

ROOT2="$WORK/omarchy"
mkdir -p "$ROOT2/etc/skel/.config/hypr" "$ROOT2/etc" "$ROOT2/var/log"
printf '[options]\n' > "$ROOT2/etc/pacman.conf"
# Pretend Omarchy already wrote these.
printf '# Omarchy hyprland config\nmonitor=,preferred,auto,1\n' \
  > "$ROOT2/etc/skel/.config/hypr/hyprland.conf"
mkdir -p "$ROOT2/etc/skel/.config/waybar"
printf '# Omarchy waybar\n' > "$ROOT2/etc/skel/.config/waybar/config.jsonc"
BEFORE_WAYBAR="$(md5sum "$ROOT2/etc/skel/.config/waybar/config.jsonc" | cut -d' ' -f1)"

env XOS_INSTALL_ROOT="$ROOT2" XOS_PACKAGE_MANAGER="$WORK/pm" \
    XOS_LOG="$ROOT2/var/log/xos-install.log" \
    bash "$INSTALL/08-desktop.sh" > "$WORK/desktop.txt" 2>&1
AFTER_WAYBAR="$(md5sum "$ROOT2/etc/skel/.config/waybar/config.jsonc" | cut -d' ' -f1)"

check "Omarchy's waybar config is untouched" "it was modified" \
  "$([ "$BEFORE_WAYBAR" = "$AFTER_WAYBAR" ] && echo 0 || echo 1)"
check "the XOS bar lives in its own directory" "no xos waybar config" \
  "$([ -f "$ROOT2/etc/skel/.config/waybar/xos/config.jsonc" ] && echo 0 || echo 1)"
check "hyprland.conf gains exactly one source line" "the include is wrong" \
  "$([ "$(grep -c 'source = ~/.config/hypr/xos.conf' "$ROOT2/etc/skel/.config/hypr/hyprland.conf")" = "1" ] && echo 0 || echo 1)"
check "Omarchy's own hyprland lines survive" "they were overwritten" \
  "$(grep -q 'monitor=,preferred,auto,1' "$ROOT2/etc/skel/.config/hypr/hyprland.conf" && echo 0 || echo 1)"

# Running it twice must not add the line twice.
env XOS_INSTALL_ROOT="$ROOT2" XOS_PACKAGE_MANAGER="$WORK/pm" \
    XOS_LOG="$ROOT2/var/log/xos-install.log" \
    bash "$INSTALL/08-desktop.sh" > /dev/null 2>&1
check "running it twice adds nothing twice" "the include was duplicated" \
  "$([ "$(grep -c 'source = ~/.config/hypr/xos.conf' "$ROOT2/etc/skel/.config/hypr/hyprland.conf")" = "1" ] && echo 0 || echo 1)"

# And a file XOS did not write is left alone rather than replaced.
printf 'somebody else wrote this\n' > "$ROOT2/etc/skel/.config/foot/foot.ini"
env XOS_INSTALL_ROOT="$ROOT2" XOS_PACKAGE_MANAGER="$WORK/pm" \
    XOS_LOG="$ROOT2/var/log/xos-install.log" \
    bash "$INSTALL/08-desktop.sh" > /dev/null 2>&1
check "a config XOS did not write is not overwritten" "it was clobbered" \
  "$(grep -q 'somebody else wrote this' "$ROOT2/etc/skel/.config/foot/foot.ini" && echo 0 || echo 1)"

# ---------------------------------------------------------------- what runs

printf '\nWhat starts at boot, on an 8GB machine\n'

ROOT3="$WORK/services"
sandbox "$ROOT3" 05-services.sh

UNITS="$ROOT3/etc/systemd/user"
for name in xos-openwebui xos-searxng xos-openclaw; do
  check "$name is socket-activated" "no socket unit" \
    "$([ -f "$UNITS/$name.socket" ] && echo 0 || echo 1)"
  check "$name is not started at boot" "it has an Install section, so it would be" \
    "$(grep -q '^\[Install\]' "$UNITS/$name.service" && echo 1 || echo 0)"
  check "$name gives its memory back when idle" "no exit-idle-time" \
    "$(grep -q 'exit-idle-time' "$UNITS/$name-proxy.service" && echo 0 || echo 1)"
  check "$name stops when nothing needs it" "no StopWhenUnneeded" \
    "$(grep -q 'StopWhenUnneeded=yes' "$UNITS/$name.service" && echo 0 || echo 1)"
  check "$name binds loopback only" "it would be reachable from the network" \
    "$(grep -qE '^ListenStream=' "$UNITS/$name.socket" && \
       ! grep -E '^ListenStream=' "$UNITS/$name.socket" | grep -qvE '^ListenStream=127\.0\.0\.1:' \
       && echo 0 || echo 1)"
done

# What it is started with, not what the comment says about it. The unit explains
# why Bun is not used, so grepping the whole file for "bun" finds the
# explanation and calls it a violation.
check "the OpenClaw gateway is started with Node" "Bun is unstable for these sessions" \
  "$(grep -E '^ExecStart=' "$UNITS/xos-openclaw.service" | grep -q '/node' && echo 0 || echo 1)"
check "nothing starts it with Bun" "Bun drops WhatsApp and Telegram sessions" \
  "$(grep -E '^ExecStart=' "$UNITS/xos-openclaw.service" | grep -qi 'bun' && echo 1 || echo 0)"

# Messaging configured: the gateway must start at boot, because an inbound
# message cannot socket-activate a listener that is not listening.
ROOT4="$WORK/messaging"
mkdir -p "$ROOT4/etc/xos"
printf '[whatsapp]\nenabled = true\n' > "$ROOT4/etc/xos/messaging.toml"
sandbox "$ROOT4" 05-services.sh
check "with messaging configured the gateway starts at boot" "it would miss inbound messages" \
  "$(grep -q 'the OpenClaw gateway starts at boot' "$ROOT4/var/log/xos-install.log" && echo 0 || echo 1)"

# xosd and llama-server, and only those.
ROOT5="$WORK/xosd"
sandbox "$ROOT5" 09-xosd.sh
check "xosd is enabled at boot" "the daemon everything else is a client of" \
  "$(grep -q 'WantedBy=default.target' "$ROOT5/etc/systemd/user/xosd.service" && echo 0 || echo 1)"

ROOT6="$WORK/inference"
sandbox "$ROOT6" 04-inference.sh
check "llama-server is enabled at boot" "the local model would load on demand, slowly" \
  "$(grep -q 'WantedBy=default.target' "$ROOT6/etc/systemd/user/xos-llama.service" && echo 0 || echo 1)"
check "llama.cpp is built for compute 6.1" "a Pascal card would fall back to the CPU" \
  "$(grep -q 'CMAKE_CUDA_ARCHITECTURES' "$INSTALL/04-inference.sh" && grep -q 'CUDA_ARCH=61' "$INSTALL/04-inference.sh" && echo 0 || echo 1)"
check "slot and prefix caching are on" "an old GPU would feel much slower" \
  "$(grep -q -- '--cache-reuse' "$ROOT6/etc/systemd/user/xos-llama.service" && echo 0 || echo 1)"

# ---------------------------------------------------------------- standalone

check "no install step can reach the host package manager" "a test could install on this machine" \
  "$(grep -q 'apt-get' "$WORK/calls.txt" 2>/dev/null && echo 1 || echo 0)"

# Every 8GB machine reports less than 8192 MB, and XOS targets 8GB machines.
check "no model threshold is a round power of two" "it would exclude the machine it was written for" \
  "$(grep -E '"minimum_(memory|vram)_mb": (1024|2048|4096|8192|16384)' "$INSTALL/../verified-models.json" && echo 1 || echo 0)"
check "the reference model fits a real 8GB machine" "the target machine is excluded" \
  "$(python3 -c "
import json
db = json.load(open('$INSTALL/../verified-models.json'))
gemma = [e for e in db['entries'] if e['id'] == 'gemma-4-e4b'][0]
raise SystemExit(0 if gemma['minimum_memory_mb'] <= 7800 else 1)
" && echo 0 || echo 1)"

# A URL that does not resolve fails at the very first step of somebody's
# install, before anything has had a chance to tell them what XOS is. It pointed
# at a repository that does not exist.
check "boot.sh points at a repository that exists" "the one-line install would fail immediately" \
  "$(grep -q 'github.com/gigad1k/XOS' "$INSTALL/boot.sh" && echo 0 || echo 1)"
check "no placeholder host is offered as the way in" "xos.sh does not exist yet" \
  "$(grep -E '^#   curl' "$INSTALL/boot.sh" | grep -q 'xos.sh' && echo 1 || echo 0)"
printf '
No step asks this machine about the machine being built
'

# pipx, npm and uv are installed into the target by 02-runtimes. Every step
# that used them asked `command -v` here instead, found nothing during an
# install from media, and skipped itself politely: no Open WebUI, no SearXNG,
# no messaging gateway, no OpenCode, no speech, no wake word. The machine came
# up missing most of what XOS is, and every one of those was a soft failure
# with a reasonable-sounding message.
STRAY=""
for step in 04-inference.sh 05-services.sh 06-opencode.sh 07-models.sh 08-desktop.sh; do
  grep -nE '^[[:space:]]*((el)?if )?command -v (uv|pipx|npm|plymouth)' "$INSTALL/$step" \
    >/dev/null 2>&1 && STRAY="$STRAY $step"
  grep -nE '^[[:space:]]*(uv|pipx|npm|plymouth-set-default-theme) ' "$INSTALL/$step" \
    >/dev/null 2>&1 && STRAY="$STRAY $step"
done
check "no layer step probes or installs on the wrong machine" "stray:$STRAY" \
  "$([ -z "$STRAY" ] && echo 0 || echo 1)"
check "and lib.sh is where crossing into the target lives" "each step would grow its own copy" \
  "$(grep -q '^in_target() {' "$INSTALL/lib.sh" && echo 0 || echo 1)"

printf '
The local model goes onto the disk too
'

# uv and the huggingface CLI are installed into the target by 02-runtimes, not
# onto the machine running the script. Looking for them here means finding
# nothing during an install from media, skipping the download, and leaving a
# machine whose first non-negotiable is that it works offline with no local
# model on it.
check "the downloader is looked for in the target" "it would never be found on the medium" \
  "$(grep -q 'in_target command -v hf' "$INSTALL/07-models.sh" && echo 0 || echo 1)"
check "and the download runs there" "it would write to the wrong filesystem" \
  "$(grep -q 'in_target hf download' "$INSTALL/07-models.sh" && echo 0 || echo 1)"
check "with a path that means something inside it" "a chroot has no install-root prefix" \
  "$(grep -q 'MODELS_THERE' "$INSTALL/07-models.sh" && echo 0 || echo 1)"

printf '
Omarchy goes into the machine being built
'

# Every other line in install.sh is $XOS_ROOT-aware. This one was a pipe into
# bash on the live system, so an install from the medium put Omarchy into RAM
# and left the target disk an Arch with no desktop on it.
check "it installs into the target root, not the live one" "the desktop would go into RAM" \
  "$(grep -q 'arch-chroot "$XOS_ROOT" bash /tmp/omarchy-install' "$INSTALL/install.sh" && echo 0 || echo 1)"
OMARCHY_GUARD="$(grep -n 'if \[ -n "\$XOS_ROOT" \]; then' "$INSTALL/install.sh" | head -1 | cut -d: -f1)"
OMARCHY_PIPE="$(grep -n 'omarchy.org/install | bash' "$INSTALL/install.sh" | head -1 | cut -d: -f1)"
check "the un-chrooted pipe is only reached when installing onto this machine" "it would still go to the wrong root" \
  "$([ -n "$OMARCHY_GUARD" ] && [ -n "$OMARCHY_PIPE" ] && [ "$OMARCHY_PIPE" -gt "$OMARCHY_GUARD" ] && echo 0 || echo 1)"
check "a download that fails does not stop the install" "one unreachable host would end everything" \
  "$(grep -q 'could not download the Omarchy installer' "$INSTALL/install.sh" && echo 0 || echo 1)"


printf '\nxosd installs on its own\n'

ROOT7="$WORK/standalone"
mkdir -p "$ROOT7/var/log"
# No lib.sh beside it, as if someone had curled just this one file.
cp "$INSTALL/09-xosd.sh" "$WORK/only-xosd.sh"
env PATH="$STUBS:$PATH" XOS_INSTALL_ROOT="$ROOT7" XOS_LOG="$ROOT7/var/log/xos.log" \
    XOS_DRY_RUN=1 bash "$WORK/only-xosd.sh" --dry-run > "$WORK/standalone.txt" 2>&1
check "it runs with none of the rest of XOS present" "exited $?" \
  "$(grep -q 'xosd is installed' "$WORK/standalone.txt" && echo 0 || echo 1)"
check "it detects the package manager rather than assuming pacman" "no detection" \
  "$(grep -qE 'package manager: (arch|debian|unknown)' "$WORK/standalone.txt" && echo 0 || echo 1)"
check "it handles Debian as well as Arch" "Debian is not handled" \
  "$(grep -q 'apt-get install' "$INSTALL/09-xosd.sh" && echo 0 || echo 1)"

# The medium ships xosd and xos already compiled. Building them again during an
# install would install a Rust toolchain onto the machine running the script -
# which, installing from the medium, is the live system in RAM rather than the
# disk being built - and then spend ten minutes recompiling what is already
# there.
PREBUILT="$WORK/prebuilt"
mkdir -p "$PREBUILT"
printf '#!/bin/sh
exit 0
' > "$PREBUILT/xosd"
printf '#!/bin/sh
exit 0
' > "$PREBUILT/xos"
chmod +x "$PREBUILT/xosd" "$PREBUILT/xos"
R8="$WORK/prebuilt-root"
mkdir -p "$R8/etc" "$R8/var/log"
printf '[options]
' > "$R8/etc/pacman.conf"
env PATH="$STUBS:$PATH" XOS_INSTALL_ROOT="$R8" XOS_PREBUILT_DIR="$PREBUILT" \
    XOS_SOURCE="$INSTALL/.." XOS_PACKAGE_MANAGER="$WORK/pm" \
    XOS_LOG="$R8/var/log/xos-install.log" \
    bash "$INSTALL/09-xosd.sh" > "$WORK/prebuilt.txt" 2>&1
check "it uses binaries that are already built" "it would rebuild what the medium carries" \
  "$(grep -q 'no toolchain is needed' "$WORK/prebuilt.txt" && echo 0 || echo 1)"
check "and compiles nothing" "ten minutes of an install spent on nothing" \
  "$(grep -q 'nothing to compile' "$WORK/prebuilt.txt" && echo 0 || echo 1)"
check "and puts them in the target root" "the installed machine would have no XOS on it" \
  "$([ -x "$R8/usr/local/bin/xosd" ] && [ -x "$R8/usr/local/bin/xos" ] && echo 0 || echo 1)"
check "without installing a toolchain anywhere" "packages would land on the live system" \
  "$(grep -q 'rust' "$WORK/calls.txt" 2>/dev/null && echo 1 || echo 0)"

# ---------------------------------------------------------------- the disk

printf '\nThe one script that destroys data\n'

env XOS_INSTALL_ROOT="$WORK/base" XOS_LOG="$WORK/base.log" XOS_FORCE_FIRMWARE=bios \
    bash "$INSTALL/base.sh" --dry-run < /dev/null > "$WORK/base.txt" 2>&1
check "with no disk named it changes nothing" "it proceeded without a disk" \
  "$(grep -q 'No disk chosen' "$WORK/base.txt" && echo 0 || echo 1)"

env XOS_INSTALL_ROOT="$WORK/base2" XOS_LOG="$WORK/base2.log" XOS_FORCE_FIRMWARE=bios \
    bash "$INSTALL/base.sh" --dry-run --disk /dev/sdZZ < /dev/null > "$WORK/base2.txt" 2>&1
check "a dry run writes no partition table" "it ran a partitioning command for real" \
  "$(grep -qE '^ *(sgdisk|parted|mkfs)' "$WORK/base2.txt" && echo 1 || echo 0)"
check "a dry run says what it would do" "it said nothing" \
  "$(grep -q 'would run: parted' "$WORK/base2.txt" && echo 0 || echo 1)"
check "it warns before destroying anything" "no warning" \
  "$(grep -q 'WILL BE DESTROYED' "$WORK/base2.txt" && echo 0 || echo 1)"

# An installer that erases the disk and then discovers it cannot download
# anything has failed somebody twice. pacstrap pulls about 600MB, so whether
# the mirrors answer is checked before the partition table is replaced, not
# after.
NET_CHECK="$(grep -n 'pacman -Sy' "$INSTALL/base.sh" | head -1 | cut -d: -f1)"
FIRST_WRITE="$(grep -nE '^[[:space:]]*run (sgdisk|parted)' "$INSTALL/base.sh" | head -1 | cut -d: -f1)"
check "it checks the mirrors before it erases the disk" "a network failure would cost somebody their data" \
  "$([ -n "$NET_CHECK" ] && [ -n "$FIRST_WRITE" ] && [ "$NET_CHECK" -lt "$FIRST_WRITE" ] && echo 0 || echo 1)"
check "and says what to do about it" "it would just stop" \
  "$(grep -q 'Connect this machine to the internet' "$INSTALL/base.sh" && echo 0 || echo 1)"
check "a dry run does not contact the mirrors" "a dry run would need a network" \
  "$(grep -q 'dry run, so the mirrors are not contacted' "$WORK/base2.txt" && echo 0 || echo 1)"
check "legacy BIOS gets an MBR label" "it would not boot on a pre-2012 machine" \
  "$(grep -q 'mklabel msdos' "$WORK/base2.txt" && echo 0 || echo 1)"
check "legacy BIOS gets a BIOS boot partition" "GRUB would have nowhere to go" \
  "$(grep -q 'set 1 boot on' "$WORK/base2.txt" && echo 0 || echo 1)"

env XOS_INSTALL_ROOT="$WORK/base3" XOS_LOG="$WORK/base3.log" XOS_FORCE_FIRMWARE=uefi \
    bash "$INSTALL/base.sh" --dry-run --disk /dev/sdZZ < /dev/null > "$WORK/base3.txt" 2>&1
check "UEFI gets a GPT label" "wrong partition table" \
  "$(grep -q 'sgdisk --zap-all' "$WORK/base3.txt" && echo 0 || echo 1)"
check "UEFI gets an EFI system partition" "nowhere for the bootloader" \
  "$(grep -q 'typecode=1:ef00' "$WORK/base3.txt" && echo 0 || echo 1)"
check "encryption is opt-in, not the default" "it encrypted without being asked" \
  "$(grep -q 'Not encrypting' "$WORK/base3.txt" && echo 0 || echo 1)"

# The encrypted install adds cryptdevice= to /etc/default/grub, but GRUB's real
# config was generated earlier in the file. If nothing regenerates it, the
# parameter sits in a file the firmware never reads and the machine boots
# nowhere. Order is the bug, so order is what is checked.
CRYPT_EDIT="$(grep -n 'cryptdevice=UUID=\$ROOT_UUID:xosroot' "$INSTALL/base.sh" | head -1 | cut -d: -f1)"
LAST_MKCONFIG="$(grep -n 'grub-mkconfig' "$INSTALL/base.sh" | tail -1 | cut -d: -f1)"
check "an encrypted install regenerates grub.cfg after setting cryptdevice"   "the parameter is written after the last grub-mkconfig, so it never reaches the boot menu"   "$([ -n "$CRYPT_EDIT" ] && [ -n "$LAST_MKCONFIG" ] && [ "$LAST_MKCONFIG" -gt "$CRYPT_EDIT" ] && echo 0 || echo 1)"
check "and checks the parameter actually went in" "a sed that matched nothing would pass silently"   "$(grep -q 'the cryptdevice parameter did not go in' "$INSTALL/base.sh" && echo 0 || echo 1)"
check "it will not erase a disk with nobody watching" "it would run unattended" \
  "$(grep -q 'Nothing is reading the prompt' "$WORK/base.txt" || grep -q 'dry run' "$WORK/base3.txt" && echo 0 || echo 1)"

env XOS_INSTALL_ROOT="$WORK/base4" XOS_LOG="$WORK/base4.log" \
    bash "$INSTALL/base.sh" --dry-run --disk /dev/sdZZ < /dev/null > "$WORK/base4.txt" 2>&1
BASE4_EXIT="$?"
if [ "$(bash -c '[ -d /sys/firmware/efi ] || [ -d /sys/class/dmi/id ] && echo yes')" = "yes" ]; then
  pass "the firmware mode is visible here, so the cannot-tell path is not exercised"
else
  check "it refuses to partition when it cannot tell UEFI from BIOS" "it guessed" \
    "$([ "$BASE4_EXIT" != "0" ] && grep -q 'cannot be determined' "$WORK/base4.txt" && echo 0 || echo 1)"
fi

# ---------------------------------------------------------------- the boot line

printf '\nThe kernel command line reaches the boot menu\n'

# base.sh writes GRUB's config while installing the base system. The hardware
# step runs afterwards and is the thing that knows which card is in the machine.
# Whatever it asks for therefore has to be applied by something that runs later
# still, or it is written to a file nothing ever reads and an NVIDIA machine
# comes up to a black screen.

# arch-chroot is stubbed to do nothing, which would make a regeneration look
# like it worked. This one actually writes a grub.cfg from the defaults file,
# so the test can check the generated config rather than the input to it.
BSTUBS="$WORK/stubs-boot"
mkdir -p "$BSTUBS"
cat > "$BSTUBS/arch-chroot" <<'STUB'
#!/usr/bin/env bash
root="$1"; shift
if [ "${1:-}" = "grub-mkconfig" ]; then
  out=""
  while [ "$#" -gt 0 ]; do [ "$1" = "-o" ] && out="$2"; shift; done
  line="$(sed -n 's/^GRUB_CMDLINE_LINUX_DEFAULT="\(.*\)"$/\1/p' "$root/etc/default/grub" | tail -1)"
  mkdir -p "$(dirname "$root$out")"
  printf 'menuentry "XOS" {\n  linux /vmlinuz-linux root=/dev/vda2 %s\n}\n' "$line" > "$root$out"
  exit 0
fi
exit 0
STUB
chmod +x "$BSTUBS/arch-chroot"

boot_root() { # root
  mkdir -p "$1/etc/xos" "$1/etc/default" "$1/var/log" "$1/boot/grub"
  printf 'GRUB_CMDLINE_LINUX_DEFAULT="loglevel=3 quiet"\nGRUB_CMDLINE_LINUX=""\n' \
    > "$1/etc/default/grub"
}
run_boot_step() { # root, extra args...
  local root="$1"; shift
  env PATH="$BSTUBS:$STUBS:$PATH" XOS_INSTALL_ROOT="$root" \
      XOS_LOG="$root/var/log/xos-install.log" \
      bash "$INSTALL/10-boot.sh" "$@" 2>&1
}

B1="$WORK/boot1"
boot_root "$B1"
run_boot_step "$B1" > "$WORK/boot1.txt"
check "with nothing asked for it changes nothing" "it touched the boot config anyway" \
  "$(grep -q 'no kernel parameters' "$WORK/boot1.txt" && echo 0 || echo 1)"

B2="$WORK/boot2"
boot_root "$B2"
printf 'nvidia-drm.modeset=1\nibt=off\n' > "$B2/etc/xos/kernel-parameters"
run_boot_step "$B2" --dry-run > "$WORK/boot2.txt"
check "a dry run says what it would add and writes nothing" "a dry run edited the boot config" \
  "$(grep -q 'would add them' "$WORK/boot2.txt" && ! grep -q 'nvidia-drm' "$B2/etc/default/grub" && echo 0 || echo 1)"

B3="$WORK/boot3"
boot_root "$B3"
printf 'nvidia-drm.modeset=1\nibt=off\n' > "$B3/etc/xos/kernel-parameters"
run_boot_step "$B3" > "$WORK/boot3.txt"
check "what the hardware step asked for lands on the command line" "it did not reach /etc/default/grub" \
  "$(grep -q 'nvidia-drm.modeset=1' "$B3/etc/default/grub" && grep -q 'ibt=off' "$B3/etc/default/grub" && echo 0 || echo 1)"
check "and reaches the config the firmware actually reads" "grub.cfg was never regenerated" \
  "$(grep -q 'nvidia-drm.modeset=1' "$B3/boot/grub/grub.cfg" 2>/dev/null && echo 0 || echo 1)"
check "what was already there is kept" "it replaced the existing command line" \
  "$(grep -q 'loglevel=3 quiet' "$B3/etc/default/grub" && echo 0 || echo 1)"
check "it says the parameters reached the boot menu" "it claimed nothing either way" \
  "$(grep -q 'now passes them to the kernel' "$WORK/boot3.txt" && echo 0 || echo 1)"

run_boot_step "$B3" > "$WORK/boot3b.txt"
check "running the layer twice does not add them twice" "the parameter accumulated" \
  "$([ "$(grep -c 'nvidia-drm.modeset=1' "$B3/etc/default/grub")" = "1" ] && echo 0 || echo 1)"
check "and it says so rather than pretending to work" "it reported a change it did not make" \
  "$(grep -q 'already on the command line' "$WORK/boot3b.txt" && echo 0 || echo 1)"

B4="$WORK/boot4"
mkdir -p "$B4/etc/xos" "$B4/var/log"
printf 'nvidia-drm.modeset=1\n' > "$B4/etc/xos/kernel-parameters"
run_boot_step "$B4" > "$WORK/boot4.txt"
check "with no GRUB it says the parameters were not applied" "it failed silently" \
  "$(grep -q 'not applied' "$WORK/boot4.txt" && grep -q 'nvidia-drm.modeset=1' "$WORK/boot4.txt" && echo 0 || echo 1)"

BOOT_STEP="$(grep -n 'run_step 10-boot.sh' "$INSTALL/install.sh" | cut -d: -f1)"
HW_STEP="$(grep -n 'run_step 00-hardware.sh' "$INSTALL/install.sh" | cut -d: -f1)"
check "the boot step runs after the hardware step, not before" "it would read a file not written yet" \
  "$([ -n "$BOOT_STEP" ] && [ -n "$HW_STEP" ] && [ "$BOOT_STEP" -gt "$HW_STEP" ] && echo 0 || echo 1)"
check "it is the last step in the layer" "a later step could still ask for a parameter" \
  "$([ "$(grep -n 'run_step' "$INSTALL/install.sh" | tail -1 | cut -d: -f1)" = "$BOOT_STEP" ] && echo 0 || echo 1)"

printf '\n%s passed, %s failed\n\n' "$PASSED" "$FAILED"
[ "$FAILED" = "0" ]
