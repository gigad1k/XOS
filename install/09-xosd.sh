#!/usr/bin/env bash
#
# Build and install xosd.
#
# # This one stands alone
#
# Every other script here assumes the XOS install. This one does not, on
# purpose: it has to work on somebody's existing Arch or Debian box, run by
# itself, with none of the rest of XOS present. XOS is the turnkey version of
# this idea, not the only version, and a daemon that can only exist inside a
# whole-machine install is a daemon almost nobody will try.
#
# So: it detects the package manager rather than assuming pacman, installs its
# own build dependencies, and does not touch the desktop, the bootloader or
# anything else.
#
#   curl -fsSL .../09-xosd.sh | bash

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Standalone means the shared library might not be there.
if [ -f "$HERE/lib.sh" ]; then
  # shellcheck source=lib.sh
  . "$HERE/lib.sh"
else
  XOS_ROOT="${XOS_INSTALL_ROOT:-/}"; XOS_ROOT="${XOS_ROOT%/}"
  XOS_DRY_RUN="${XOS_DRY_RUN:-0}"
  XOS_LOG="${XOS_LOG:-/tmp/xos-install.log}"
  xlog() { printf '%s\n' "$*" | tee -a "$XOS_LOG" >/dev/null; printf '%s\n' "$*"; }
  xstep() { xlog ""; xlog "== $*"; }
  xwarn() { xlog "!! $*"; }
  xsoft_fail() { xwarn "$1"; return 0; }
  layer_into() { local d="$XOS_ROOT/$1"; mkdir -p "$(dirname "$d")" 2>/dev/null; cat > "$d"; xlog "   wrote $1"; }
  install_user_unit() { layer_into "etc/systemd/user/$1"; }
  enable_user_unit() { systemctl --user enable "$1" >/dev/null 2>&1 || xsoft_fail "could not enable $1"; }
fi

for argument in "$@"; do
  [ "$argument" = "--dry-run" ] && XOS_DRY_RUN=1
done

SOURCE="${XOS_SOURCE:-$HERE/..}"
PREFIX="${XOS_PREFIX:-$XOS_ROOT/usr/local}"

# ---------------------------------------------------------------- the host

xstep "This machine"

FAMILY="unknown"
if [ -n "${XOS_PACKAGE_MANAGER:-}" ]; then
  FAMILY="substituted"
elif command -v pacman >/dev/null 2>&1; then
  FAMILY="arch"
elif command -v apt-get >/dev/null 2>&1; then
  FAMILY="debian"
fi
xlog "   package manager: $FAMILY"

install_build_dependencies() {
  [ "$XOS_DRY_RUN" = "1" ] && { xlog "   would install the build dependencies"; return 0; }

  # Whatever the caller substituted, if anything. Reaching past that to the real
  # package manager means a run aimed at another root installs on this one
  # instead, and a test run as root installs on the machine running the tests.
  if [ -n "${XOS_PACKAGE_MANAGER:-}" ]; then
    xlog "   using $XOS_PACKAGE_MANAGER"
    "$XOS_PACKAGE_MANAGER" -S --noconfirm --needed base-devel rust git pkgconf openssl sqlite \
      >> "$XOS_LOG" 2>&1
    return $?
  fi

  case "$FAMILY" in
    arch)
      pacman -S --noconfirm --needed base-devel rust git pkgconf openssl sqlite \
        >> "$XOS_LOG" 2>&1
      ;;
    debian)
      apt-get update >> "$XOS_LOG" 2>&1
      apt-get install -y build-essential cargo git pkg-config libssl-dev libsqlite3-dev \
        >> "$XOS_LOG" 2>&1
      ;;
    *)
      xsoft_fail "unknown package manager; install a Rust toolchain yourself and rerun"
      return 1
      ;;
  esac
}

# ---------------------------------------------------------------- prebuilt

# Already built beats building, and on install media it is the only right
# answer.
#
# The medium carries xosd and xos compiled at build time, precisely so that
# installing does not need a Rust toolchain. Building here anyway does two wrong
# things at once: pacman -S rust installs onto the machine running the script,
# which during an install from the medium is the live system in RAM and not the
# disk being built, and then it spends ten minutes of somebody's install
# recompiling what is already sitting in /usr/local/bin.
#
# So when there is another root to install into, prebuilt binaries are the whole
# of it, and a medium without them says so rather than quietly fetching a
# compiler into memory.
PREBUILT_XOSD=""
PREBUILT_XOS=""

find_prebuilt() {
  local name="$1" candidate
  for candidate in "${XOS_PREBUILT_DIR:+$XOS_PREBUILT_DIR/$name}" \
                   "$SOURCE/target/release/$name" \
                   "/usr/local/bin/$name" \
                   "/usr/bin/$name"
  do
    [ -n "$candidate" ] || continue
    [ -x "$candidate" ] || continue
    # Never the file this run is about to write.
    [ "$candidate" = "$PREFIX/bin/$name" ] && continue
    printf '%s' "$candidate"
    return 0
  done
  return 1
}

if [ -n "$XOS_ROOT" ]; then
  PREBUILT_XOSD="$(find_prebuilt xosd || true)"
  PREBUILT_XOS="$(find_prebuilt xos || true)"
fi

if [ -n "$PREBUILT_XOSD" ] && [ -n "$PREBUILT_XOS" ]; then
  xstep "Prebuilt binaries"
  xlog "   $PREBUILT_XOSD"
  xlog "   $PREBUILT_XOS"
  xlog "   no toolchain is needed, and nothing is compiled"
elif [ -n "$XOS_ROOT" ] && [ "$XOS_DRY_RUN" != "1" ] && ! command -v cargo >/dev/null 2>&1; then
  xstep "Prebuilt binaries"
  xwarn "there is no built xosd here and no toolchain to build one"
  xlog "   If this is install media, it was built with --skip-binaries: build"
  xlog "   it again without that. Otherwise install a Rust toolchain, or run"
  xlog "   this script on the installed machine where one can be installed"
  xlog "   somewhere that does not disappear at the next reboot."
  exit 1
else
  xstep "Build dependencies"
  install_build_dependencies || xwarn "build dependencies may be incomplete"
fi

# ---------------------------------------------------------------- build

xstep "Building"

if [ -n "$PREBUILT_XOSD" ] && [ -n "$PREBUILT_XOS" ]; then
  xlog "   already built; nothing to compile"
elif [ "$XOS_DRY_RUN" = "1" ]; then
  xlog "   would build xosd and xos in release mode from $SOURCE"
else
  if ! command -v cargo >/dev/null 2>&1; then
    xwarn "no cargo on PATH, so xosd cannot be built"
    xlog "   install a Rust toolchain and run this again"
    exit 1
  fi
  # Release, not debug. This runs all day on a machine chosen for being old.
  ( cd "$SOURCE" && cargo build --release --locked ) >> "$XOS_LOG" 2>&1 \
    || { xwarn "xosd did not build"; exit 1; }
fi

xstep "Installing"

INSTALLED=1

if [ "$XOS_DRY_RUN" = "1" ]; then
  xlog "   would install xosd and xos into $PREFIX/bin"
else
  mkdir -p "$PREFIX/bin" 2>/dev/null
  FROM_XOSD="${PREBUILT_XOSD:-$SOURCE/target/release/xosd}"
  FROM_XOS="${PREBUILT_XOS:-$SOURCE/target/release/xos}"
  install -m 0755 "$FROM_XOSD" "$PREFIX/bin/xosd" 2>/dev/null \
    || { xwarn "could not install xosd from $FROM_XOSD"; INSTALLED=0; }
  install -m 0755 "$FROM_XOS" "$PREFIX/bin/xos" 2>/dev/null \
    || { xwarn "could not install xos from $FROM_XOS"; INSTALLED=0; }
fi

# The hardware database travels with the daemon, because a daemon that cannot
# resolve a driver is a daemon that has to guess.
if [ -f "$SOURCE/hardware-db.json" ]; then
  if [ "$XOS_DRY_RUN" = "1" ]; then
    xlog "   would install hardware-db.json"
  else
    mkdir -p "$XOS_ROOT/usr/share/xos" 2>/dev/null
    install -m 0644 "$SOURCE/hardware-db.json" "$XOS_ROOT/usr/share/xos/hardware-db.json" 2>/dev/null
    [ -f "$SOURCE/verified-models.json" ] && \
      install -m 0644 "$SOURCE/verified-models.json" "$XOS_ROOT/usr/share/xos/verified-models.json" 2>/dev/null
    xlog "   installed the hardware and model databases"
  fi
fi

# ---------------------------------------------------------------- the unit

xstep "The unit"

install_user_unit "xosd.service" <<'UNIT'
# Written by XOS.
#
# One of only two units XOS starts at boot. Everything else in the system is a
# client of this daemon, so it being up is the difference between XOS existing
# and not.
[Unit]
Description=XOS daemon
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/xosd
Restart=on-failure
RestartSec=3
# It should lose to anything the person is actually looking at.
Nice=3

[Install]
WantedBy=default.target
UNIT

enable_user_unit "xosd.service"

# A user unit that should run without somebody logged in. Without this, the
# daemon stops the moment the session ends, which on a machine meant to work in
# the background is the opposite of the point.
if [ "$XOS_DRY_RUN" = "1" ]; then
  xlog "   would enable lingering so xosd runs without a session"
else
  loginctl enable-linger "${SUDO_USER:-$USER}" >> "$XOS_LOG" 2>&1 \
    || xsoft_fail "could not enable lingering; xosd will stop when you log out"
fi

xlog ""
if [ "$INSTALLED" = "1" ]; then
  xlog "   xosd is installed. It works on its own: this script needs none of the"
  xlog "   rest of XOS, and nothing else on this machine has been changed."
  xlog "   Start it now with: systemctl --user start xosd"
  exit 0
fi

# Saying it is installed after failing to install it is the kind of line
# somebody believes, reboots on, and then has to work out for themselves.
xwarn "xosd was NOT installed. The unit is in place but there is no binary for it."
xlog "   The build or the copy failed; the reason is above, in $XOS_LOG."
xlog "   Fix that and run this script again. Nothing else has been changed."
exit 1
