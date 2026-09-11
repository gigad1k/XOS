#!/usr/bin/env bash
#
# The one line that installs XOS.
#
#   curl -fsSL https://raw.githubusercontent.com/gigad1k/XOS/main/install/boot.sh | bash
#
# (There is no xos.sh yet. When there is, it will serve this file.)
#
# It works out where it is and does the right thing from there. On the live ISO
# with nothing installed, that means partitioning a disk and putting Arch on it
# first. On a machine that already runs Arch, it means applying the XOS layer
# and leaving the rest alone.
#
# Getting that wrong in either direction is bad in a way nothing else here is:
# partitioning a machine somebody is already using would destroy their data. So
# the test is conservative, and when it cannot tell, it asks rather than
# assuming.

set -uo pipefail

REPOSITORY="${XOS_REPOSITORY:-https://github.com/gigad1k/XOS}"
BRANCH="${XOS_BRANCH:-main}"
WORK="${XOS_WORK:-/tmp/xos-install}"

say() { printf '%s\n' "$*"; }

say ""
say "  XOS"
say "  An AI-native Linux for the machine you already have."
say ""

# ---------------------------------------------------------------- get it

if [ -f "$(dirname "$0")/install.sh" ]; then
  # Running from a checkout already.
  HERE="$(cd "$(dirname "$0")" && pwd)"
else
  if ! command -v git >/dev/null 2>&1; then
    say "  git is needed to fetch XOS, and it is not here."
    say "  On the Arch ISO: pacman -Sy git"
    exit 1
  fi
  rm -rf "$WORK" 2>/dev/null
  say "  Fetching XOS..."
  git clone --depth 1 --branch "$BRANCH" "$REPOSITORY" "$WORK" >/dev/null 2>&1 \
    || { say "  Could not fetch $REPOSITORY"; exit 1; }
  HERE="$WORK/install"
fi

# ---------------------------------------------------------------- where are we

# An installed system has a root filesystem that is not the ISO's, and a real
# /etc/fstab. The live environment has neither.
LIVE=0
if [ -d /run/archiso ]; then
  LIVE=1
elif [ ! -s /etc/fstab ] && [ ! -d /home ]; then
  LIVE=1
fi

if [ "$LIVE" = "1" ]; then
  say "  This looks like the install media."
  say ""
  say "  XOS will partition a disk and install Arch on it, then layer itself on"
  say "  top. That destroys everything on the disk you choose."
  say ""
  say "  Look at what is here first:"
  say ""
  lsblk -dno NAME,SIZE,MODEL 2>/dev/null | sed 's/^/    /'
  say ""
  say "  Then run:"
  say ""
  say "    $HERE/base.sh --dry-run --disk /dev/sdX     # plan it, change nothing"
  say "    $HERE/base.sh --disk /dev/sdX               # do it, after confirming"
  say "    $HERE/install.sh                            # apply the XOS layer"
  say ""
  say "  base.sh is not run for you. It is the only thing in XOS that destroys"
  say "  data, and it should be typed by somebody who has read that line."
  exit 0
fi

# ---------------------------------------------------------------- layer it

say "  This machine already has a system on it, so nothing will be partitioned."
say "  XOS will install its layer: drivers, runtimes, tools, the local model,"
say "  the desktop and the daemon."
say ""

if [ -t 0 ]; then
  printf '  Go ahead? [y/N] '
  read -r answer
  case "$answer" in
    [Yy]*) ;;
    *) say "  Nothing has been changed."; exit 0 ;;
  esac
else
  say "  Running unattended. Pass --dry-run to see what would happen instead."
fi

exec bash "$HERE/install.sh" "$@"
