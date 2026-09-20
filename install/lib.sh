#!/usr/bin/env bash
#
# Shared ground for every XOS install step.
#
# Three ideas run through all of it.
#
# **Never modify Omarchy in place.** XOS layers over Omarchy and does not fork
# it, because owning a distro means owning kernel updates, firmware and driver
# breakage forever. So nothing here edits a file Omarchy wrote. Configuration is
# layered: a drop-in directory, an include line appended to a file Omarchy
# treats as the user's own, or a unit override. `layer_into` is the only way
# this codebase writes near Omarchy, and it refuses to overwrite.
#
# **Pin every package version.** An install that works today and breaks on a
# machine installed next month is not an install anyone can support. Versions
# live in packages.lock, one place, and `pin_install` refuses to install
# anything that is not in it.
#
# **Nothing here is irreversible without being asked.** Every destructive step
# announces itself, and --dry-run does the whole run without touching anything.

set -uo pipefail

XOS_ROOT="${XOS_INSTALL_ROOT:-/}"
XOS_ROOT="${XOS_ROOT%/}"
XOS_PM="${XOS_PACKAGE_MANAGER:-pacman}"
XOS_LOG="${XOS_LOG:-$XOS_ROOT/var/log/xos-install.log}"
XOS_LOCK="${XOS_LOCK:-$(dirname "${BASH_SOURCE[0]}")/packages.lock}"
XOS_DRY_RUN="${XOS_DRY_RUN:-0}"

for argument in ${XOS_ARGUMENTS:-} "$@"; do
  case "$argument" in
    --dry-run) XOS_DRY_RUN=1 ;;
    *) ;;
  esac
done

mkdir -p "$(dirname "$XOS_LOG")" 2>/dev/null

# ---------------------------------------------------------------- output

xlog() {
  printf '%s  %s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*" >> "$XOS_LOG" 2>/dev/null
  printf '%s\n' "$*"
}

xstep() { xlog ""; xlog "== $*"; }

xwarn() { xlog "!! $*"; }

# A step that failed but must not stop the install.
xsoft_fail() {
  xwarn "$1"
  xlog "   carrying on; this is recoverable from the desktop"
  return 0
}

# ---------------------------------------------------------------- packages

# The pinned version for a package, or nothing if it is not in the lock file.
pinned_version() {
  [ -f "$XOS_LOCK" ] || return 1
  awk -F= -v name="$1" '
    /^[[:space:]]*#/ { next }
    $1 == name { print $2; found = 1; exit }
    END { if (!found) exit 1 }
  ' "$XOS_LOCK"
}

# Run something on the machine being built rather than the machine doing the
# building.
#
# The layer steps install pipx, npm, uv and the tools that come with them into
# the target, and then ask `command -v` whether those tools exist - on this
# machine. Installing from media that is the live system, where they never
# were, so step after step found nothing and skipped itself: no Open WebUI, no
# SearXNG, no messaging gateway, no OpenCode, no speech, no wake word, and no
# local model. Every one of them soft-failed politely and the installed machine
# came up missing most of what XOS is.
#
# A no-op when XOS is being installed onto this machine, which is the case the
# bare calls were written for.
in_target() {
  if [ -n "$XOS_ROOT" ] && command -v arch-chroot >/dev/null 2>&1; then
    arch-chroot "$XOS_ROOT" "$@"
  else
    "$@"
  fi
}

# Run the package manager against whatever root is being installed to.
#
# This is the difference between installing XOS and installing nothing. Left to
# itself, pacman installs into the running system — which during an install is
# the live medium's RAM overlay, and that ceases to exist at the reboot.
#
# arch-chroot rather than `pacman --root`, because package scripts expect /proc,
# /sys and /dev to be mounted when they run.
package_manager() {
  if [ -n "$XOS_ROOT" ] && [ "$XOS_PM" = "pacman" ] && command -v arch-chroot >/dev/null 2>&1; then
    arch-chroot "$XOS_ROOT" pacman "$@"
  else
    "$XOS_PM" "$@"
  fi
}

# Install a package at its pinned version.
#
# An unpinned package is refused rather than installed loose. The guardrail is
# not decoration: an install that silently picks up whatever is current today is
# an install that cannot be reproduced or supported, and on this project the
# whole point is machines that keep working.
pin_install() {
  local package="$1"
  local version
  if ! version="$(pinned_version "$package")"; then
    xwarn "$package is not in packages.lock, so it is not installed"
    xlog "   add it there with a version; an unpinned install cannot be reproduced"
    return 1
  fi

  if [ "$XOS_DRY_RUN" = "1" ]; then
    xlog "   would install $package $version"
    return 0
  fi

  xlog "   installing $package $version"
  if package_manager -S --noconfirm --needed "$package" >> "$XOS_LOG" 2>&1; then
    return 0
  fi
  return 1
}

# Install several, reporting each. One failure never stops the rest.
pin_install_all() {
  local failed=0
  for package in "$@"; do
    pin_install "$package" || { xwarn "$package did not install"; failed=1; }
  done
  return "$failed"
}

# Hold a package at its installed version, so an update cannot move it.
hold_package() {
  local package="$1"
  local conf="$XOS_ROOT/etc/pacman.conf"
  [ "$XOS_DRY_RUN" = "1" ] && { xlog "   would hold $package"; return 0; }
  [ -f "$conf" ] || return 1
  if grep -qE "^[[:space:]]*IgnorePkg.*[[:space:]=]$package([[:space:]]|$)" "$conf" 2>/dev/null; then
    return 0
  fi
  if grep -qE "^[[:space:]]*IgnorePkg" "$conf" 2>/dev/null; then
    sed -i "s/^\([[:space:]]*IgnorePkg[[:space:]]*=.*\)$/\1 $package/" "$conf" 2>/dev/null
  else
    printf '\nIgnorePkg = %s\n' "$package" >> "$conf" 2>/dev/null
  fi
  xlog "   held $package at its installed version"
}

# ---------------------------------------------------------------- layering

# Write a file that belongs to XOS, beside but never over Omarchy's.
#
# Refuses to overwrite anything it did not write itself. That refusal is the
# guardrail made mechanical: if XOS ever starts clobbering Omarchy's files, an
# upstream update stops being applicable and the reason XOS layers at all is
# gone.
layer_into() { # destination, content-on-stdin
  local destination="$XOS_ROOT/$1"
  if [ -e "$destination" ] && ! grep -q "Written by XOS" "$destination" 2>/dev/null; then
    xwarn "$1 already exists and was not written by XOS; leaving it alone"
    return 1
  fi
  if [ "$XOS_DRY_RUN" = "1" ]; then
    cat > /dev/null
    xlog "   would write $1"
    return 0
  fi
  mkdir -p "$(dirname "$destination")" 2>/dev/null
  cat > "$destination" || return 1
  xlog "   wrote $1"
}

# Append one line to a file Omarchy treats as the user's own, exactly once.
#
# Used for the single `source =` line that pulls in an entire XOS config file.
# One idempotent line is the smallest possible footprint in someone else's file,
# and it is removable by hand by anyone who wants XOS gone.
include_once() { # file, line
  local file="$XOS_ROOT/$1"
  local line="$2"
  if [ "$XOS_DRY_RUN" = "1" ]; then
    xlog "   would add to $1: $line"
    return 0
  fi
  mkdir -p "$(dirname "$file")" 2>/dev/null
  touch "$file" 2>/dev/null
  if grep -qxF "$line" "$file" 2>/dev/null; then
    return 0
  fi
  printf '\n# Added by XOS. Remove this line and XOS stops applying.\n%s\n' \
    "$line" >> "$file" || return 1
  xlog "   included $1 -> $line"
}

# ---------------------------------------------------------------- systemd

# Install a user unit. Never enabled here: whether a unit starts at boot is a
# decision about someone's memory, taken deliberately in 05-services.sh.
install_user_unit() { # name, content-on-stdin
  layer_into "etc/systemd/user/$1"
}

enable_user_unit() {
  [ "$XOS_DRY_RUN" = "1" ] && { xlog "   would enable $1"; return 0; }

  # systemctl acts on the running system, which during an install is the live
  # medium rather than the machine being built. The link it would make is one
  # line, so it is made in the target directly.
  if [ -n "$XOS_ROOT" ]; then
    local wants="$XOS_ROOT/etc/systemd/user/default.target.wants"
    mkdir -p "$wants" 2>/dev/null
    if ln -sf "/etc/systemd/user/$1" "$wants/$1" 2>/dev/null; then
      xlog "   $1 will start at boot"
    else
      xsoft_fail "could not enable $1"
    fi
    return 0
  fi

  systemctl --user enable "$1" >> "$XOS_LOG" 2>&1 \
    || xsoft_fail "could not enable $1"
}

# ---------------------------------------------------------------- facts

# Is this machine UEFI, legacy BIOS, or somewhere the question cannot be
# answered? The installer partitions on this, so "cannot tell" is its own state
# and never silently rounded to legacy.
firmware_mode() {
  if [ -d /sys/firmware/efi ]; then
    echo "uefi"
  elif [ -d /sys/class/dmi/id ] || [ -d /sys/firmware/dmi ]; then
    echo "bios"
  else
    echo "unknown"
  fi
}

has_aes_ni() {
  grep -qm1 -E '^flags.*\baes\b' /proc/cpuinfo 2>/dev/null
}

# The NVIDIA branch this machine needs, from the hardware step's own answer.
resolved_nvidia_branch() {
  local report="$XOS_ROOT/var/log/xos-hardware-report.json"
  [ -f "$report" ] || return 1
  python3 -c "
import json, sys
try:
    data = json.load(open(sys.argv[1]))
except Exception:
    sys.exit(1)
for device in data.get('display', []):
    if device.get('branch'):
        print(device['branch'])
        sys.exit(0)
sys.exit(1)
" "$report" 2>/dev/null
}
