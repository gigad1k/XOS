#!/usr/bin/env bash
#
# Apply the XOS layer to a machine that already has Arch on it.
#
# Order matters, and it is the order of the numbered files.
#
# 00-hardware.sh runs first, before anything else at all, because everything
# after it depends on knowing what the machine is. The driver step reads its
# answer rather than deciding again; the model step reads its profile rather
# than guessing what will fit.
#
# Omarchy runs between the hardware step and the XOS layer: it wants a fresh
# Arch install, and the XOS layer wants Omarchy to already be there to layer
# over.
#
# A step that fails does not stop the run. By this point the machine boots, and
# a desktop missing one tool is recoverable by the person sitting at it. What is
# not recoverable is an install that stopped in the middle and left a machine
# nobody can describe.

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$HERE/lib.sh"

SKIP_OMARCHY="${XOS_SKIP_OMARCHY:-0}"
ONLY=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --dry-run) XOS_DRY_RUN=1; export XOS_DRY_RUN; shift ;;
    --skip-omarchy) SKIP_OMARCHY=1; shift ;;
    --only) ONLY="${2:-}"; shift 2 ;;
    *) shift ;;
  esac
done

export XOS_INSTALL_ROOT="$XOS_ROOT"
export XOS_PACKAGE_MANAGER="$XOS_PM"
export XOS_LOG
export XOS_DRY_RUN

run_step() {
  local script="$HERE/$1"
  if [ -n "$ONLY" ] && [ "$ONLY" != "$1" ]; then
    return 0
  fi
  if [ ! -f "$script" ]; then
    xwarn "$1 is missing"
    return 0
  fi

  # Tested against the value, not merely for the variable being set.
  # `${XOS_DRY_RUN:+--dry-run}` expands whenever the variable is non-empty, and
  # it is always "0" or "1" and so never empty: every step was told --dry-run,
  # every real install would have installed nothing, reported success, and left
  # somebody rebooting into a machine with none of this on it.
  local flags=()
  [ "$XOS_DRY_RUN" = "1" ] && flags=(--dry-run)

  xstep "$1"
  # A step that fails is reported and the run continues. Stopping halfway is
  # the one outcome nobody can recover from.
  if bash "$script" ${flags[@]+"${flags[@]}"}; then
    xlog "   $1 finished"
  else
    xwarn "$1 reported a problem; carrying on"
  fi
}

xstep "XOS layer"
xlog "   root: ${XOS_ROOT:-/}"
[ "$XOS_DRY_RUN" = "1" ] && xlog "   dry run: nothing will be installed or written"

# Before everything. The rest of the install reads what this writes.
run_step 00-hardware.sh

# ---------------------------------------------------------------- Omarchy

if [ -n "$ONLY" ]; then
  : # a single step was asked for, so Omarchy is not part of it
elif [ "$SKIP_OMARCHY" = "1" ]; then
  xstep "Omarchy"
  xlog "   skipped, as asked"
elif [ -d "$XOS_ROOT/etc/omarchy" ] || [ -d "$HOME/.local/share/omarchy" ]; then
  xstep "Omarchy"
  xlog "   already installed; XOS layers over what is there"
else
  xstep "Omarchy"
  # Run, never forked and never patched. Owning a distro means owning kernel
  # updates, firmware and driver breakage forever, which is a full-time job that
  # has nothing to do with what XOS is for.
  if [ "$XOS_DRY_RUN" = "1" ]; then
    xlog "   would run the Omarchy installer"
  elif command -v curl >/dev/null 2>&1; then
    curl -fsSL https://omarchy.org/install | bash >> "$XOS_LOG" 2>&1 \
      || xsoft_fail "the Omarchy installer did not finish; the XOS layer will still apply"
  else
    xsoft_fail "no curl, so Omarchy was skipped"
  fi
fi

# ---------------------------------------------------------------- the layer

run_step 01-drivers.sh
run_step 02-runtimes.sh
run_step 03-tools.sh
run_step 04-inference.sh
run_step 05-services.sh
run_step 06-opencode.sh
run_step 07-models.sh
run_step 08-desktop.sh
run_step 09-xosd.sh

xstep "Done"
xlog ""
xlog "   The XOS layer is applied. Nothing Omarchy owns was modified."
xlog "   The whole log is at $XOS_LOG."
xlog ""
xlog "   Reboot, and the desktop comes up with xosd running and the bar live."
exit 0
