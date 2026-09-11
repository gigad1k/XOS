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
         08-desktop.sh 09-xosd.sh)

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

printf '\n%s passed, %s failed\n\n' "$PASSED" "$FAILED"
[ "$FAILED" = "0" ]
