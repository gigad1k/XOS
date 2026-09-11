#!/usr/bin/env bash
#
# The XOS installer, as it runs on the XOS install media.
#
# One thing to run. It works out what is in the machine, asks the handful of
# questions that genuinely change the outcome, and then partitions, installs
# Arch, applies the XOS layer and reboots — without anyone having to assemble
# three commands from a manual.
#
# # What it still asks
#
# Everything here is designed to need as little from a person as possible,
# except the one thing that must never be automatic: which disk to erase. That
# is asked plainly, shown in full, and confirmed by typing the name back. An
# installer that guesses a disk is a data-loss bug with a progress bar.
#
# # Why it starts a daemon
#
# Driver resolution lives in xosd, and `xos` is a thin client over it. On live
# media there is no daemon, so without this the whole hardware step would fall
# back to "no inventory available" and the machine would be installed with no
# driver resolution at all — on exactly the hardware that most needs it. The
# media carries prebuilt binaries; this starts one, uses it, and stops it.
#
#   ./live.sh                 the guided install
#   ./live.sh --dry-run       every question, no writes
#   ./live.sh --disk /dev/sda --yes    unattended, for a VM

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$HERE/lib.sh"

DISK=""
ENCRYPT=0
ASSUME_YES=0
HOSTNAME_WANTED="xos"
REBOOT=1

while [ "$#" -gt 0 ]; do
  case "$1" in
    --disk) DISK="${2:-}"; shift 2 ;;
    --encrypt) ENCRYPT=1; shift ;;
    --hostname) HOSTNAME_WANTED="${2:-xos}"; shift 2 ;;
    --yes) ASSUME_YES=1; shift ;;
    --no-reboot) REBOOT=0; shift ;;
    --dry-run) XOS_DRY_RUN=1; export XOS_DRY_RUN; shift ;;
    *) shift ;;
  esac
done

INTERACTIVE=1
[ -t 0 ] || INTERACTIVE=0

ask_yes_no() { # question, default(0|1)
  local question="$1" default="$2" answer
  if [ "$INTERACTIVE" = "0" ] || [ "$ASSUME_YES" = "1" ]; then
    return "$([ "$default" = "1" ] && echo 0 || echo 1)"
  fi
  printf '  %s [%s] ' "$question" "$([ "$default" = "1" ] && echo 'Y/n' || echo 'y/N')"
  read -r answer
  case "$(printf '%s' "$answer" | tr '[:upper:]' '[:lower:]')" in
    y|yes) return 0 ;;
    n|no) return 1 ;;
    *) return "$([ "$default" = "1" ] && echo 0 || echo 1)" ;;
  esac
}

# ---------------------------------------------------------------- welcome

clear 2>/dev/null
cat <<'BANNER'

  XOS

  An AI-native Linux for the machine you already have.

  This installs XOS on this computer. It will:

    - work out what hardware is here and which drivers it needs
    - erase one disk that you choose, and only that one
    - install Arch Linux, then the XOS layer on top
    - reboot into a desktop with a local assistant already running

  Nothing is erased until you have seen the disk and typed its name back.

BANNER

[ "${XOS_DRY_RUN:-0}" = "1" ] && echo "  Dry run: every question, no writes." && echo

if [ "$INTERACTIVE" = "1" ] && [ "$ASSUME_YES" != "1" ]; then
  if ! ask_yes_no "Go on?" 1; then
    echo
    echo "  Nothing has been changed."
    exit 0
  fi
fi

# ---------------------------------------------------------------- the daemon

xstep "Looking at this machine"

XOS_BIN="$(command -v xos || true)"
XOSD_BIN="$(command -v xosd || true)"
DAEMON_PID=""
INVENTORY_FILE=""

start_daemon() {
  [ -n "$XOSD_BIN" ] || return 1
  local runtime="${XDG_RUNTIME_DIR:-/run}"
  export XOS_SOCKET="${runtime%/}/xosd.sock"
  rm -f "$XOS_SOCKET" 2>/dev/null
  "$XOSD_BIN" >/tmp/xosd-live.log 2>&1 &
  DAEMON_PID="$!"
  local waited=0
  while [ "$waited" -lt 60 ]; do
    [ -S "$XOS_SOCKET" ] && return 0
    sleep 0.25
    waited=$((waited + 1))
  done
  return 1
}

stop_daemon() {
  [ -n "$DAEMON_PID" ] || return 0
  kill "$DAEMON_PID" 2>/dev/null
  wait "$DAEMON_PID" 2>/dev/null
  DAEMON_PID=""
}
trap stop_daemon EXIT

if [ -n "$XOS_BIN" ] && start_daemon; then
  INVENTORY_FILE="$(mktemp)"
  if "$XOS_BIN" hardware --json > "$INVENTORY_FILE" 2>/dev/null && [ -s "$INVENTORY_FILE" ]; then
    export XOS_HARDWARE_JSON="$INVENTORY_FILE"
    "$XOS_BIN" hardware 2>/dev/null | sed 's/^/  /'
  else
    # Not fatal. The install carries on and the hardware step falls back to
    # what is safe on any machine, which is exactly what it is designed to do.
    xwarn "the hardware inventory could not be read; drivers will be resolved conservatively"
    INVENTORY_FILE=""
  fi
else
  xwarn "no XOS binaries on this media, so drivers will be resolved conservatively"
  xlog "   the install still works; it just knows less about this machine"
fi

# ---------------------------------------------------------------- the disk

xstep "Choosing a disk"

list_disks() {
  lsblk -dpno NAME,SIZE,MODEL,TYPE 2>/dev/null |
    awk '$NF == "disk" { $NF = ""; print }' |
    grep -vE '^/dev/(loop|ram|zram|sr)' || true
}

DISKS="$(list_disks)"
if [ -z "$DISKS" ]; then
  xwarn "no disks found on this machine"
  xlog "   nothing has been changed"
  exit 1
fi

echo
echo "$DISKS" | nl -w4 -s'  ' | sed 's/^/  /'
echo

if [ -z "$DISK" ]; then
  if [ "$INTERACTIVE" = "0" ]; then
    # A destructive step must never pick for somebody who is not there.
    xwarn "no disk chosen, and nothing is reading the prompt"
    xlog "   pass --disk /dev/sdX. Nothing has been changed."
    exit 1
  fi
  printf '  Which disk? (number, or a name like /dev/sda): '
  read -r chosen
  case "$chosen" in
    /dev/*) DISK="$chosen" ;;
    ''|*[!0-9]*) xwarn "that is not a disk"; exit 1 ;;
    *) DISK="$(echo "$DISKS" | sed -n "${chosen}p" | awk '{print $1}')" ;;
  esac
fi

if [ -z "$DISK" ]; then
  xwarn "no disk chosen. Nothing has been changed."
  exit 1
fi

# ---------------------------------------------------------------- encryption

xstep "Encryption"

if has_aes_ni; then
  xlog "   This CPU has AES-NI, so encryption costs almost nothing."
  if [ "$ENCRYPT" = "0" ] && ask_yes_no "Encrypt the disk?" 1; then
    ENCRYPT=1
  fi
else
  # Opt-in rather than mandatory, and this is the reason. Many of the machines
  # XOS targets predate AES-NI, and software encryption on them turns an
  # already slow machine into an unusable one.
  xlog "   This CPU has no AES-NI. Encryption would be done in software and"
  xlog "   would make this machine noticeably slower at everything that"
  xlog "   touches the disk. On hardware this old the honest answer is no."
  if [ "$ENCRYPT" = "0" ] && ask_yes_no "Encrypt anyway?" 0; then
    ENCRYPT=1
  fi
fi
xlog "   encryption: $([ "$ENCRYPT" = "1" ] && echo "yes" || echo "no")"

# ---------------------------------------------------------------- name

if [ "$INTERACTIVE" = "1" ] && [ "$ASSUME_YES" != "1" ]; then
  xstep "Name"
  printf '  What should this machine be called? [%s] ' "$HOSTNAME_WANTED"
  read -r typed
  [ -n "$typed" ] && HOSTNAME_WANTED="$typed"
fi

# ---------------------------------------------------------------- the plan

xstep "The plan"

FIRMWARE="$(firmware_mode)"
case "$FIRMWARE" in
  bios)    FIRMWARE_NOTE="legacy, which XOS supports and most installers do not" ;;
  uefi)    FIRMWARE_NOTE="" ;;
  *)       FIRMWARE_NOTE="cannot be read here, and partitioning needs a definite answer" ;;
esac

xlog ""
xlog "   disk         $DISK"
xlog "   firmware     $FIRMWARE${FIRMWARE_NOTE:+  ($FIRMWARE_NOTE)}"
xlog "   encryption   $([ "$ENCRYPT" = "1" ] && echo "yes" || echo "no")"
xlog "   name         $HOSTNAME_WANTED"
xlog ""

if [ "$FIRMWARE" = "unknown" ]; then
  # Said here, where the plan is, rather than a screen later after somebody has
  # already agreed to it.
  xwarn "This will stop before it partitions anything."
  xlog "   XOS refuses to guess a partition table: a guess produces a machine"
  xlog "   that installs perfectly and then does not boot. Run this on the real"
  xlog "   machine, or set XOS_FORCE_FIRMWARE=uefi or bios if you are certain."
  xlog ""
fi

xlog "   Then: Arch, the desktop, the tools, a local model, and the XOS daemon."
xlog "   That takes a while and most of it is downloading."
xlog ""

# ---------------------------------------------------------------- do it

# base.sh asks about encryption too, for anyone running it on its own. That
# conversation has already happened here, so it is told not to repeat it.
export XOS_ENCRYPTION_SETTLED=1

BASE_ARGS=(--disk "$DISK" --hostname "$HOSTNAME_WANTED")
[ "$ENCRYPT" = "1" ] && BASE_ARGS+=(--encrypt)
[ "$ASSUME_YES" = "1" ] && BASE_ARGS+=(--yes)
[ "${XOS_DRY_RUN:-0}" = "1" ] && BASE_ARGS+=(--dry-run)

xstep "Installing"
# base.sh does its own confirmation: it shows the disk, says everything on it
# will be destroyed, and requires the name typed back. That prompt is not
# duplicated here, because two confirmations for one action teach people to
# click through both.
if ! bash "$HERE/base.sh" "${BASE_ARGS[@]}"; then
  xwarn "the base install did not finish"
  xlog "   Nothing further has been done. The log is at $XOS_LOG."
  exit 1
fi

LAYER_ARGS=()
[ "${XOS_DRY_RUN:-0}" = "1" ] && LAYER_ARGS+=(--dry-run)

# The XOS layer goes on inside the new system, not on the live media.
export XOS_INSTALL_ROOT="${XOS_MOUNT:-/mnt}"
bash "$HERE/install.sh" ${LAYER_ARGS[@]+"${LAYER_ARGS[@]}"}

# ---------------------------------------------------------------- finished

xstep "Done"
xlog ""
xlog "   XOS is installed on $DISK."
xlog ""
xlog "   When it comes up it will run a short setup, and then do one small"
xlog "   thing by itself so you can watch it work. Every step of that can be"
xlog "   skipped, and XOS works either way."
xlog ""

stop_daemon
[ -n "$INVENTORY_FILE" ] && rm -f "$INVENTORY_FILE"

if [ "${XOS_DRY_RUN:-0}" = "1" ]; then
  xlog "   That was a dry run. Nothing was written."
  exit 0
fi

if [ "$REBOOT" = "1" ]; then
  if [ "$ASSUME_YES" = "1" ] || ask_yes_no "Reboot into XOS now?" 1; then
    xlog "   Take the install media out as it restarts."
    sleep 3
    systemctl reboot 2>/dev/null || reboot
  fi
fi
exit 0
