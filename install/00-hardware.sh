#!/usr/bin/env bash
#
# XOS install step 00 — hardware.
#
# This runs before every other install step, because nothing else matters if the
# machine will not boot afterwards. That is the governing rule here and it is
# absolute: never fail to boot. A degraded desktop is recoverable by the person
# sitting at it. A black screen is not.
#
# Two consequences run through the whole file.
#
# First, no step may abort the install. There is no `set -e`, deliberately, and
# every command that can fail is wrapped so that a failure is logged, falls back,
# and carries on. An installer that stops halfway leaves a machine in a state
# nobody asked for.
#
# Second, a guess is worse than a fallback. If a device is not in
# hardware-db.json, XOS does not pick a driver that looks close. It takes the
# next level down the chain, which is something that works on anything.
#
# Testing an installer matters as much as writing one, so three environment
# variables exist to let it run somewhere other than a real install:
#
#   XOS_INSTALL_ROOT   prefix for everything written (default /)
#   XOS_PACKAGE_MANAGER  the install command (default pacman)
#   XOS_HARDWARE_JSON  a captured inventory instead of asking the daemon
#
# and --dry-run resolves and reports without installing anything.

set -uo pipefail

# ---------------------------------------------------------------- settings

ROOT="${XOS_INSTALL_ROOT:-/}"
ROOT="${ROOT%/}"
PACKAGE_MANAGER="${XOS_PACKAGE_MANAGER:-pacman}"
LOG="$ROOT/var/log/xos-hardware.log"
PACMAN_CONF="$ROOT/etc/pacman.conf"
KERNEL_PARAMETERS_FILE="$ROOT/etc/xos/kernel-parameters"
REPORT_JSON="$ROOT/var/log/xos-hardware-report.json"
# Bundled DKMS sources live on the install media. They have to: the AUR needs
# the internet that the wifi driver is supposed to be providing.
#
# Several places, because the answer depends on how XOS got here. The XOS
# medium carries them in its own filesystem overlay; a plain Arch ISO with the
# packages copied alongside puts them on the mounted image; an installed system
# keeps them where the rest of XOS lives. Naming only one of those is how the
# drivers get built onto a medium and then never found.
find_dkms_dir() {
  local candidate
  for candidate in     "${XOS_DKMS_DIR:-}"     /root/xos/dkms     /run/archiso/bootmnt/xos/dkms     /usr/share/xos/dkms
  do
    [ -n "$candidate" ] || continue
    # A directory with nothing in it is not a source of drivers.
    if [ -d "$candidate" ] && [ -n "$(ls -A "$candidate" 2>/dev/null | grep -v '^README$')" ]; then
      printf '%s' "$candidate"
      return 0
    fi
  done
  printf '%s' "${XOS_DKMS_DIR:-/root/xos/dkms}"
  return 1
}
DKMS_DIR="$(find_dkms_dir)"

DRY_RUN=0
# The environment can say it too. An unattended install reaches this file
# through install.sh, which passes flags of its own choosing, so the answer to
# "is anybody reading this" has to be able to arrive some other way.
ASSUME_NO="${XOS_ASSUME_NO:-0}"
for argument in "$@"; do
  case "$argument" in
    --dry-run) DRY_RUN=1 ;;
    --assume-no) ASSUME_NO=1 ;;
    *) ;;
  esac
done

mkdir -p "$(dirname "$LOG")" 2>/dev/null
mkdir -p "$(dirname "$KERNEL_PARAMETERS_FILE")" 2>/dev/null
: > "$LOG" 2>/dev/null || LOG=/dev/null

# What the report will say, built up as we go.
REPORT_LINES=()
KERNEL_PARAMETERS=()
WIFI_STATE="not applicable"
WIFI_ADVICE=""

# ---------------------------------------------------------------- output

log() {
  # Everything goes to the log; the person watching sees it too. If they have to
  # ring someone about this machine later, the log is what they will read.
  printf '%s  %s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*" >> "$LOG" 2>/dev/null
  printf '%s\n' "$*"
}

step() { log ""; log "== $*"; }

# A device, the driver chosen for it, and how far down the chain we had to go.
record() {
  REPORT_LINES+=("$1")
  log "   -> $1"
}

# ---------------------------------------------------------------- packages

installed() {
  if [ "$PACKAGE_MANAGER" = "pacman" ]; then
    pacman -Qq "$1" >/dev/null 2>&1
  else
    "$PACKAGE_MANAGER" -Qq "$1" >/dev/null 2>&1
  fi
}

# Install a package, saying plainly whether it worked. Never fatal.
install_package() {
  local package="$1"
  [ -z "$package" ] && return 1
  [ "$package" = "in-tree" ] && return 0
  [ "$package" = "unknown" ] && return 1

  if [ "$DRY_RUN" = "1" ]; then
    log "   would install $package"
    return 0
  fi

  if installed "$package"; then
    log "   $package is already here"
    return 0
  fi

  log "   installing $package"
  if "$PACKAGE_MANAGER" -S --noconfirm --needed "$package" >> "$LOG" 2>&1; then
    return 0
  fi
  log "   $package did not install; moving down the chain"
  return 1
}

# Pin a package so a later update cannot replace it with a version that does not
# support the card. This is the difference between a machine that keeps working
# and one that goes black after an update three months from now.
pin_package() {
  local package="$1"
  [ "$DRY_RUN" = "1" ] && { log "   would pin $package"; return 0; }
  [ -f "$PACMAN_CONF" ] || { log "   no pacman.conf to pin $package in"; return 1; }

  if grep -qE "^[[:space:]]*IgnorePkg.*[[:space:]=]$package([[:space:]]|$)" "$PACMAN_CONF" 2>/dev/null; then
    log "   $package is already pinned"
    return 0
  fi
  if grep -qE "^[[:space:]]*IgnorePkg" "$PACMAN_CONF" 2>/dev/null; then
    sed -i "s/^\([[:space:]]*IgnorePkg[[:space:]]*=.*\)$/\1 $package/" "$PACMAN_CONF" 2>/dev/null
  else
    printf '\n# Pinned by XOS: this branch is the one that supports the card in\n# this machine, and a newer one does not.\nIgnorePkg = %s\n' \
      "$package" >> "$PACMAN_CONF" 2>/dev/null
  fi
  log "   pinned $package against updates"
}

add_kernel_parameter() {
  for existing in ${KERNEL_PARAMETERS[@]+"${KERNEL_PARAMETERS[@]}"}; do
    [ "$existing" = "$1" ] && return 0
  done
  KERNEL_PARAMETERS+=("$1")
  log "   kernel parameter: $1"
}

# ---------------------------------------------------------------- inventory

step "Reading the machine"

INVENTORY=""
if [ -n "${XOS_HARDWARE_JSON:-}" ] && [ -f "$XOS_HARDWARE_JSON" ]; then
  INVENTORY="$(cat "$XOS_HARDWARE_JSON" 2>/dev/null)"
  log "   inventory from $XOS_HARDWARE_JSON"
elif command -v xos >/dev/null 2>&1; then
  INVENTORY="$(xos hardware --json 2>/dev/null)"
  log "   inventory from xos hardware --json"
fi

if [ -z "$INVENTORY" ]; then
  # Without an inventory there is nothing to resolve, and guessing is exactly
  # what this file exists to avoid. Install what is safe for anything and let
  # the desktop come up on a framebuffer.
  log "   no inventory available; installing only what is safe on any machine"
  INVENTORY='{}'
fi

# A small query helper so the rest reads as prose rather than as python.
query() {
  printf '%s' "$INVENTORY" | python3 -c "
import json, sys
try:
    data = json.load(sys.stdin)
except Exception:
    sys.exit(0)
$1
" 2>/dev/null
}

# ---------------------------------------------------------------- firmware

step "Firmware"
# Unconditional, on every machine. These are the packages whose absence produces
# the most confusing failures: a wifi card that scans but never associates, or
# a laptop with no sound and no error anywhere.
for package in linux-firmware sof-firmware; do
  if install_package "$package"; then
    record "firmware: $package installed"
  else
    record "firmware: $package FAILED — some devices may not work"
  fi
done

# ---------------------------------------------------------------- graphics

step "Graphics"

GPU_COUNT="$(query 'print(len(data.get("inventory", {}).get("gpus", [])))')"
GPU_COUNT="${GPU_COUNT:-0}"

if [ "$GPU_COUNT" = "0" ]; then
  record "display: none detected — the desktop will come up on a framebuffer"
fi

# Hybrid graphics: the display is driven from the integrated GPU and the
# discrete card is left headless for compute. Doing it the other way round is
# how a laptop ends up with an unusable desktop and a hot fan.
INTEGRATED_INDEX="$(query '
gpus = data.get("inventory", {}).get("gpus", [])
integrated = [i for i, g in enumerate(gpus) if g.get("vendor_id") == "8086" or (g.get("vendor_id") == "1002" and g.get("vram_mb") is None)]
print(integrated[0] if integrated and len(gpus) > 1 else -1)
')"
INTEGRATED_INDEX="${INTEGRATED_INDEX:--1}"
if [ "$INTEGRATED_INDEX" != "-1" ]; then
  log "   hybrid graphics: the display comes from the integrated GPU, index $INTEGRATED_INDEX"
fi

index=0
while [ "$index" -lt "$GPU_COUNT" ]; do
  DESCRIPTION="$(query "print(data['inventory']['gpus'][$index].get('description', ''))")"
  VENDOR="$(query "print(data['inventory']['gpus'][$index].get('vendor_id', ''))")"
  DEVICE_ID="$(query "print(data['inventory']['gpus'][$index].get('device_id', ''))")"
  DRIVER="$(query "
r = data.get('resolve', {}).get('display', [])
print(r[$index].get('driver', 'unknown') if len(r) > $index else 'unknown')")"
  UTILS="$(query "
r = data.get('resolve', {}).get('display', [])
print(r[$index].get('utils') or '' if len(r) > $index else '')")"
  BRANCH="$(query "
r = data.get('resolve', {}).get('display', [])
print(r[$index].get('branch') or '' if len(r) > $index else '')")"
  CHAIN="$(query "
r = data.get('resolve', {}).get('display', [])
print(' '.join(r[$index].get('fallback', [])) if len(r) > $index else '')")"
  PARAMETERS="$(query "
r = data.get('resolve', {}).get('display', [])
print(' '.join(r[$index].get('kernel_parameters', [])) if len(r) > $index else '')")"

  log ""
  log "   $DESCRIPTION  [$VENDOR:$DEVICE_ID]"

  if [ "$INTEGRATED_INDEX" != "-1" ] && [ "$index" != "$INTEGRATED_INDEX" ]; then
    log "   this is the discrete card; leaving it headless for compute"
  fi

  LEVEL=0
  SETTLED=""

  # Level 0 is whatever the database matched. An unknown device has no level 0:
  # the guardrail is that XOS never installs a driver for a device ID it does
  # not know, because a plausible guess that does not work costs a reboot and a
  # wrong theory about why.
  if [ "$DRIVER" != "unknown" ] && [ -n "$DRIVER" ]; then
    if install_package "$DRIVER"; then
      # The kernel module is only half of it. Without the matching userspace
      # there is no libGL and no Xorg driver, and the machine boots to a black
      # screen with a perfectly healthy module loaded. If the userspace will not
      # install, this level has not worked, so we go down the chain.
      if [ -z "$UTILS" ] || install_package "$UTILS"; then
        SETTLED="$DRIVER"
        if [ "$VENDOR" = "10de" ] && [ -n "$BRANCH" ]; then
          # The branch is pinned because it is the one that supports this card,
          # and an update to a newer branch would simply stop driving it. The
          # userspace is pinned with it: a mismatched pair does not load.
          pin_package "$DRIVER"
          [ -n "$UTILS" ] && pin_package "$UTILS"
        fi
        for parameter in $PARAMETERS; do add_kernel_parameter "$parameter"; done
      else
        log "   $DRIVER installed but $UTILS did not, which is a black screen; moving on"
      fi
    fi
  else
    log "   not in hardware-db.json, so no driver is guessed at"
  fi

  # Then down the chain the database gave us, one level at a time.
  if [ -z "$SETTLED" ]; then
    for candidate in $CHAIN; do
      LEVEL=$((LEVEL + 1))
      case "$candidate" in
        nouveau|amdgpu|radeon|i915|virtio-gpu|bochs-drm|vmwgfx|vboxvideo|hyperv_drm|mgag200|ast)
          # In-tree kernel drivers. There is no package to install, so asking
          # the package manager for one would fail and log a failure for
          # something that is not a failure. Only the userspace is needed.
          if install_package "mesa"; then
            SETTLED="$candidate (in-tree)"
            break
          fi
          ;;
        modesetting|vesa)
          # The last levels are not packages at all. modesetting is part of
          # xorg-server and vesa is the kernel's own framebuffer, so both are
          # always available. This is why the chain cannot run out.
          SETTLED="$candidate"
          break
          ;;
        *)
          if install_package "$candidate"; then
            SETTLED="$candidate"
            break
          fi
          ;;
      esac
    done
  fi

  # And if even that failed, the framebuffer. There is always a picture.
  if [ -z "$SETTLED" ]; then
    LEVEL=$((LEVEL + 1))
    SETTLED="vesa"
  fi

  if [ "$LEVEL" = "0" ]; then
    record "display: $DESCRIPTION -> $SETTLED (first choice)"
  else
    record "display: $DESCRIPTION -> $SETTLED (fallback level $LEVEL)"
  fi

  index=$((index + 1))
done

# ---------------------------------------------------------------- networking

step "Networking"

NIC_COUNT="$(query 'print(len(data.get("inventory", {}).get("network", [])))')"
NIC_COUNT="${NIC_COUNT:-0}"

index=0
while [ "$index" -lt "$NIC_COUNT" ]; do
  DESCRIPTION="$(query "print(data['inventory']['network'][$index].get('description', ''))")"
  DRIVER="$(query "
r = data.get('resolve', {}).get('network', [])
print(r[$index].get('driver', 'unknown') if len(r) > $index else 'unknown')")"
  CHAIN="$(query "
r = data.get('resolve', {}).get('network', [])
print(' '.join(r[$index].get('fallback', [])) if len(r) > $index else '')")"

  log ""
  log "   $DESCRIPTION"
  SETTLED=""
  if [ "$DRIVER" = "in-tree" ]; then
    SETTLED="in-tree"
  elif [ "$DRIVER" != "unknown" ] && [ -n "$DRIVER" ]; then
    install_package "$DRIVER" && SETTLED="$DRIVER"
  else
    log "   not in hardware-db.json, so no driver is guessed at"
  fi

  if [ -z "$SETTLED" ]; then
    for candidate in $CHAIN; do
      install_package "$candidate" && { SETTLED="$candidate"; break; }
    done
  fi

  record "network: $DESCRIPTION -> ${SETTLED:-nothing that worked}"
  index=$((index + 1))
done

# Whether there is actually a wireless interface is a separate question from
# whether a package installed, and it is the one that matters.
wifi_present() {
  [ -n "${XOS_FAKE_WIFI:-}" ] && { [ "$XOS_FAKE_WIFI" = "1" ]; return; }
  for interface in /sys/class/net/*/wireless; do
    [ -e "$interface" ] && return 0
  done
  return 1
}

HAS_WIFI_HARDWARE="$(query '
nics = data.get("inventory", {}).get("network", [])
wifi = [n for n in nics if "wireless" in n.get("description", "").lower() or "wi-fi" in n.get("description", "").lower() or "wlan" in n.get("description", "").lower()]
print(1 if wifi else 0)
')"
HAS_WIFI_HARDWARE="${HAS_WIFI_HARDWARE:-0}"

if wifi_present; then
  WIFI_STATE="working"
  record "wifi: a wireless interface is up"
elif [ "$HAS_WIFI_HARDWARE" = "1" ] || [ "${XOS_FAKE_WIFI:-}" = "0" ]; then
  log ""
  log "   there is wifi hardware here but no wireless interface"
  # The DKMS sources have to be on the install media. Fetching them from the AUR
  # would need the internet that the wifi driver is supposed to provide, which
  # is the whole problem.
  TRIED=""
  if [ -d "$DKMS_DIR" ]; then
    for source in "$DKMS_DIR"/*; do
      [ -e "$source" ] || continue
      name="$(basename "$source")"
      [ "$name" = "README" ] && continue
      TRIED="$TRIED $name"
      log "   trying bundled $name"
      if [ "$DRY_RUN" = "1" ]; then
        log "   would build $name from the install media"
      else
        "$PACKAGE_MANAGER" -U --noconfirm "$source" >> "$LOG" 2>&1
      fi
      wifi_present && break
    done
  else
    log "   no bundled DKMS sources on the media at $DKMS_DIR"
  fi

  if wifi_present; then
    WIFI_STATE="working from a bundled driver"
    record "wifi: brought up by a bundled DKMS driver"
  else
    # This is the case the check is about. It must be unmistakable, and it must
    # not stop the install.
    WIFI_STATE="unavailable"
    WIFI_ADVICE="Plug in an ethernet cable, or connect a phone by USB and turn on USB tethering. XOS will work offline until then, using a local model."
    log ""
    log "   ####################################################################"
    log "   WIFI IS NOT AVAILABLE ON THIS MACHINE."
    log "   No driver here drives this wireless card.${TRIED:+ Tried:$TRIED}"
    log "   What to do: $WIFI_ADVICE"
    log "   The install is continuing. The desktop will boot."
    log "   ####################################################################"
    record "wifi: UNAVAILABLE — $WIFI_ADVICE"
  fi
else
  record "wifi: no wireless hardware found"
fi

# ---------------------------------------------------------------- bluetooth

step "Bluetooth"
if install_package "bluez" && install_package "bluez-utils"; then
  # The report is what someone reads when the machine misbehaves later, so it
  # says what happened rather than what was attempted. Claiming the service was
  # enabled two lines under "could not enable the service" is worse than saying
  # nothing at all.
  if [ "$DRY_RUN" = "1" ]; then
    log "   would enable bluetooth.service"
    record "bluetooth: bluez would be installed and the service enabled"
  elif systemctl enable bluetooth.service >> "$LOG" 2>&1; then
    record "bluetooth: bluez installed and the service enabled"
  else
    log "   could not enable bluetooth.service; continuing"
    record "bluetooth: bluez installed, but the service could not be enabled"
  fi
else
  record "bluetooth: not installed — continuing without it"
fi

# ---------------------------------------------------------------- audio

step "Audio"
AUDIO_OK=1
for package in pipewire pipewire-pulse wireplumber; do
  install_package "$package" || AUDIO_OK=0
done

sink_exists() {
  [ -n "${XOS_FAKE_SINK:-}" ] && { [ "$XOS_FAKE_SINK" = "1" ]; return; }
  command -v pactl >/dev/null 2>&1 || return 1
  pactl list short sinks 2>/dev/null | grep -q .
}

if [ "$DRY_RUN" = "1" ]; then
  record "audio: pipewire and wireplumber would be installed"
elif [ "$AUDIO_OK" = "1" ] && sink_exists; then
  record "audio: pipewire running with a sink"
else
  # Logged and continued, exactly as the guardrail says. Silence is annoying;
  # a failed install is not recoverable from the chair.
  record "audio: no sink found — sound may not work until a reboot"
fi

# ---------------------------------------------------------------- parameters

if [ "${#KERNEL_PARAMETERS[@]}" -gt 0 ]; then
  step "Kernel parameters"
  if [ "$DRY_RUN" = "1" ]; then
    log "   would write ${KERNEL_PARAMETERS[*]}"
  else
    printf '%s\n' "${KERNEL_PARAMETERS[*]}" > "$KERNEL_PARAMETERS_FILE" 2>/dev/null \
      || log "   could not write $KERNEL_PARAMETERS_FILE"
  fi
  record "kernel parameters: ${KERNEL_PARAMETERS[*]}"
fi

# ---------------------------------------------------------------- the report

step "Report"
log ""
log "   Every device, what it got, and how far down the chain it went:"
for line in ${REPORT_LINES[@]+"${REPORT_LINES[@]}"}; do
  log "     $line"
done
log ""
log "   wifi: $WIFI_STATE"
log ""
log "   This report is at $LOG"

# ------------------------------------------------- the compatibility database

# Opt-in, never automatic, and it shows exactly what would be sent before
# asking. Hardware identifiers are not especially private, but sending anything
# off a person's machine without showing it to them first is not something XOS
# does.
# The report comes from the daemon, which is the only thing that decides what
# would be sent. Two descriptions of that is one too many: the day someone adds
# a field to one of them, the other quietly keeps promising the old contents.
build_submission() {
  printf '%s' "$INVENTORY" | python3 -c "
import json, sys
try:
    data = json.load(sys.stdin)
except Exception:
    print('{}')
    sys.exit(0)
report = data.get('report')
print(json.dumps(report, indent=2) if report else '{}')
" 2>/dev/null
}

SUBMISSION="$(build_submission)"
if [ -n "$SUBMISSION" ] && [ "$SUBMISSION" != "{}" ]; then
  printf '%s\n' "$SUBMISSION" > "$REPORT_JSON" 2>/dev/null
  step "The compatibility database"
  log ""
  log "   XOS can add this machine to the community hardware database, which is"
  log "   how the next person with this card gets the right driver first time."
  log ""
  log "   This is exactly what would be sent, and nothing else:"
  log ""
  printf '%s\n' "$SUBMISSION" | sed 's/^/     /' | tee -a "$LOG" 2>/dev/null
  log ""
  log "   It is saved at $REPORT_JSON."
  if [ "$ASSUME_NO" = "1" ] || [ "$DRY_RUN" = "1" ] || [ ! -t 0 ]; then
    log "   Not sending anything. Submit it later with: xos hardware --submit"
  else
    printf '   Send it? [y/N] '
    # Bounded. A console is a tty whether or not a person is sitting at it, so
    # the check above cannot tell the difference and this question was found
    # holding an unattended install open indefinitely. Nothing is sent when it
    # expires, which is the same answer as the default and the safe one for a
    # question about data leaving the machine.
    answer=""
    if ! read -r -t 120 answer; then
      answer=""
      log ""
      log "   nobody answered, so nothing was sent"
    fi
    case "$answer" in
      [Yy]*)
        log "   sending"
        curl -fsS -X POST -H 'Content-Type: application/json' \
          --data-binary "@$REPORT_JSON" \
          https://hardware.xos.community/submit >> "$LOG" 2>&1 \
          && log "   thank you" \
          || log "   could not send it; it is still at $REPORT_JSON"
        ;;
      *) log "   not sending" ;;
    esac
  fi
fi

step "Hardware step finished"
log "   The install continues. Nothing here can stop it."
# Always. A non-zero exit from this file would abort an install over something
# that, by construction, has already fallen back to something that works.
exit 0
