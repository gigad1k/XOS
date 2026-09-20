#!/usr/bin/env bash
#
# Build the XOS install medium.
#
#   sudo ./iso/build.sh              the whole thing
#   sudo ./iso/build.sh --check      say what is missing, build nothing
#   sudo ./iso/build.sh --skip-dkms  skip the bundled wifi drivers
#
# This must run on Arch, because mkarchiso does. It is the one part of XOS with
# that requirement, and it exists so that nobody else has one: the point of a
# medium is that the person installing XOS needs no Arch machine of their own.
#
# # What it assembles
#
# Three things go onto the medium beyond a live Arch system.
#
# **The XOS source**, at /root/xos, so the installer and every script it calls
# is there with no network.
#
# **Prebuilt xos and xosd binaries.** Driver resolution lives in the daemon, and
# `xos` is a thin client over it. Without these the hardware step on live media
# would find no inventory and resolve no drivers — on exactly the machines that
# most need it. Compiling Rust on a target machine mid-install is not an
# answer; compiling it once, here, is.
#
# **The wifi drivers that are not in the kernel.** rtl8821ce, rtl8723bu and
# broadcom-wl between them cover most of the laptops that otherwise finish an
# install with no way to get online. They have to be on the medium, because
# fetching them from the AUR needs the internet the driver is supposed to be
# providing. That circularity is the whole reason this step exists.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
WORK="${XOS_ISO_WORK:-/tmp/xos-iso-work}"
OUT="${XOS_ISO_OUT:-$REPO/out}"
PROFILE="$WORK/profile"

CHECK_ONLY=0
SKIP_DKMS=0
SKIP_BINARIES=0
for argument in "$@"; do
  case "$argument" in
    --check) CHECK_ONLY=1 ;;
    --skip-dkms) SKIP_DKMS=1 ;;
    --skip-binaries) SKIP_BINARIES=1 ;;
    *) ;;
  esac
done

say()  { printf '%s\n' "$*"; }
step() { printf '\n== %s\n' "$*"; }
warn() { printf '!! %s\n' "$*"; }

# The DKMS sources to bundle. Names are AUR packages; each is fetched, built
# and dropped on the medium as a .pkg.tar.zst that 00-hardware.sh can install
# with no network.
DKMS_PACKAGES=(rtl8821ce-dkms-git rtl8723bu-dkms-git broadcom-wl-dkms)

# ---------------------------------------------------------------- what is here

step "Checking this machine can build it"

MISSING=()
need() { command -v "$1" >/dev/null 2>&1 || MISSING+=("$2"); }
need mkarchiso archiso
need pacman    pacman
need git       git

if [ "$SKIP_BINARIES" = "0" ]; then
  need cargo rust
fi
if [ "$SKIP_DKMS" = "0" ]; then
  need makepkg base-devel
fi

if [ "${#MISSING[@]}" -gt 0 ]; then
  warn "not present: ${MISSING[*]}"
  say  "   install them with: pacman -S ${MISSING[*]}"
  say  "   mkarchiso comes from the archiso package and only runs on Arch."
  [ "$CHECK_ONLY" = "1" ] || exit 1
else
  say "   archiso, pacman, git$([ "$SKIP_BINARIES" = "0" ] && echo ", cargo")$([ "$SKIP_DKMS" = "0" ] && echo ", makepkg")"
fi

if [ "$(id -u)" != "0" ] && [ "$CHECK_ONLY" = "0" ]; then
  warn "mkarchiso needs root. Run this with sudo."
  exit 1
fi

if [ "$CHECK_ONLY" = "1" ]; then
  step "Check only"
  say "   profile:   $HERE"
  say "   would build into: $WORK"
  say "   would write: $OUT"
  say "   bundled drivers: $([ "$SKIP_DKMS" = "1" ] && echo "skipped" || echo "${DKMS_PACKAGES[*]}")"
  exit 0
fi

# ---------------------------------------------------------------- the profile

step "Assembling the profile"

rm -rf "$PROFILE"
mkdir -p "$PROFILE" "$OUT"
cp -r "$HERE/." "$PROFILE/"
rm -rf "$PROFILE/build.sh" "$PROFILE/tests" "$PROFILE/README.md"
say "   $PROFILE"

# Services the live system needs. Made here rather than committed as symlinks,
# because this repository gets cloned onto filesystems that do not carry them
# and a missing symlink would mean a medium that boots with no network.
mkdir -p "$PROFILE/airootfs/etc/systemd/system/multi-user.target.wants"
ln -sf /usr/lib/systemd/system/NetworkManager.service \
       "$PROFILE/airootfs/etc/systemd/system/multi-user.target.wants/NetworkManager.service"
ln -sf /usr/lib/systemd/system/sshd.service \
       "$PROFILE/airootfs/etc/systemd/system/multi-user.target.wants/sshd.service"
# Relative, because this one is a unit the profile carries rather than one the
# package tree provides.
ln -sf ../pacman-init.service \
       "$PROFILE/airootfs/etc/systemd/system/multi-user.target.wants/pacman-init.service"
say "   NetworkManager, sshd and the pacman keyring will start on the live system"

# ---------------------------------------------------------------- XOS itself

step "Putting XOS on the medium"

SOURCE="$PROFILE/airootfs/root/xos"
mkdir -p "$SOURCE"
# The working tree as committed, so the medium carries no build output and no
# half-finished edit. tar rather than rsync for the fallback: rsync is not
# installed everywhere, and when it was missing this printed "command not
# found", said the source had been copied, and built a medium with no XOS on it.
if git -C "$REPO" rev-parse --git-dir >/dev/null 2>&1 &&
   git -C "$REPO" archive --format=tar HEAD 2>/dev/null | tar -x -C "$SOURCE" 2>/dev/null; then
  say "   source from git HEAD: $(git -C "$REPO" rev-parse --short HEAD)"
else
  tar -C "$REPO" --exclude=./target --exclude=./out --exclude=./.git -cf - . \
    | tar -C "$SOURCE" -xf - || {
      warn "could not copy the XOS source onto the medium"
      say  "   the medium would boot and have nothing to install with, so this stops here"
      exit 1
    }
  say "   source copied from the working tree"
fi

# The one thing the medium exists to carry. Checked rather than assumed,
# because the failure above was silent and produced an installer-shaped ISO
# with no installer in it.
if [ ! -f "$SOURCE/install/live.sh" ]; then
  warn "the XOS source did not land on the medium"
  say  "   $SOURCE/install/live.sh is missing, so there would be nothing to run"
  exit 1
fi
chmod +x "$SOURCE"/install/*.sh 2>/dev/null

# ---------------------------------------------------------------- binaries

if [ "$SKIP_BINARIES" = "0" ]; then
  step "Building xos and xosd"
  if ( cd "$REPO" && cargo build --release --locked ); then
    install -Dm0755 "$REPO/target/release/xosd" "$PROFILE/airootfs/usr/local/bin/xosd"
    install -Dm0755 "$REPO/target/release/xos"  "$PROFILE/airootfs/usr/local/bin/xos"
    install -Dm0644 "$REPO/hardware-db.json"    "$PROFILE/airootfs/usr/share/xos/hardware-db.json"
    install -Dm0644 "$REPO/verified-models.json" "$PROFILE/airootfs/usr/share/xos/verified-models.json"
    say "   xos and xosd are on the medium, so drivers resolve during the install"
  else
    # Not fatal, and the consequence is stated rather than discovered later.
    warn "xosd did not build"
    say  "   the medium will still install XOS, but the hardware step will"
    say  "   resolve drivers conservatively instead of from the database"
  fi
else
  warn "skipping the binaries; driver resolution during install will be conservative"
fi

# ---------------------------------------------------------------- wifi drivers

DKMS_DIR="$PROFILE/airootfs/root/xos/dkms"
mkdir -p "$DKMS_DIR"

if [ "$SKIP_DKMS" = "0" ]; then
  step "Building the wifi drivers that are not in the kernel"
  say "   These cannot be fetched during an install: getting them from the AUR"
  say "   needs the internet that they exist to provide."

  BUILD_ROOT="$WORK/dkms"
  rm -rf "$BUILD_ROOT"; mkdir -p "$BUILD_ROOT"

  # makepkg refuses to run as root, correctly, so it needs a real unprivileged
  # user with a writable home. `nobody` is not one: its home is / and nothing
  # it does can write there, so makepkg fails before it starts. Whoever ran
  # sudo is the natural choice; when there is no such person, one is made for
  # the job and removed afterwards.
  BUILDER="${SUDO_USER:-}"
  MADE_BUILDER=0
  if [ -z "$BUILDER" ] || [ "$BUILDER" = "root" ] || ! id -u "$BUILDER" >/dev/null 2>&1; then
    BUILDER=xos-build
    if ! id -u "$BUILDER" >/dev/null 2>&1; then
      useradd --system --create-home --home-dir "$BUILD_ROOT/home" \
              --shell /bin/bash "$BUILDER" 2>/dev/null
      MADE_BUILDER=1
    fi
  fi
  say "   building as $BUILDER"
  mkdir -p "$BUILD_ROOT/home"
  chown -R "$BUILDER" "$BUILD_ROOT" 2>/dev/null

  # The dependencies, installed here as root. `makepkg -s` would install them
  # itself by calling pacman through sudo, which an unprivileged builder cannot
  # do without a sudoers rule this script has no business writing.
  pacman -S --noconfirm --needed dkms git >> /dev/null 2>&1 || \
    warn "   could not install dkms; the driver packages may not build"

  BUILT=0
  for package in "${DKMS_PACKAGES[@]}"; do
    say ""
    say "   $package"
    if ! sudo -u "$BUILDER" -H git clone --depth 1 \
           "https://aur.archlinux.org/$package.git" "$BUILD_ROOT/$package" >/dev/null 2>&1; then
      warn "   could not fetch $package; skipping it"
      continue
    fi
    chown -R "$BUILDER" "$BUILD_ROOT/$package" 2>/dev/null
    # No -s: the dependencies are already in. -H so makepkg gets the builder's
    # own home rather than root's.
    if ( cd "$BUILD_ROOT/$package" &&
         sudo -u "$BUILDER" -H makepkg --noconfirm --nocheck ) > "$BUILD_ROOT/$package.log" 2>&1; then
      if cp "$BUILD_ROOT/$package"/*.pkg.tar.zst "$DKMS_DIR/" 2>/dev/null; then
        BUILT=$((BUILT + 1))
        say "   built"
      else
        warn "   $package built and produced no package file"
      fi
    else
      # The reason, not just the fact. Hunting it down afterwards means
      # rebuilding an ISO to find out.
      warn "   $package did not build:"
      tail -3 "$BUILD_ROOT/$package.log" 2>/dev/null | sed 's/^/       /'
    fi
  done

  [ "$MADE_BUILDER" = "1" ] && userdel "$BUILDER" 2>/dev/null

  say ""
  if [ "$BUILT" = "0" ]; then
    warn "no wifi drivers were bundled"
    say  "   an unsupported wifi chip will have nothing to fall back to. The"
    say  "   install still finishes and says so plainly; there will just be no"
    say  "   wifi until an ethernet cable or USB tethering is found."
  else
    say "   $BUILT bundled at /root/xos/dkms on the medium"
  fi
else
  warn "skipping the bundled wifi drivers"
fi

# 00-hardware.sh looks here on the running medium.
cat > "$PROFILE/airootfs/root/xos/dkms/README" <<'NOTE'
Wifi drivers that are not in the kernel, built into packages so they can be
installed with no network.

00-hardware.sh reaches for these when it finds wifi hardware that nothing is
driving. They have to travel on the medium: fetching them from the AUR would
need the internet that the driver is supposed to be providing.
NOTE

# ---------------------------------------------------------------- build it

step "Building the image"
say "   this takes a while and needs a few gigabytes of space"

# Cleared first. mkarchiso marks each step it finishes inside the work
# directory and skips those steps on a later run, so building twice into the
# same one does nothing whatsoever and still reports success — which is a
# miserable thing to discover while trying to fix something.
rm -rf "$WORK/mkarchiso"

if mkarchiso -v -w "$WORK/mkarchiso" -o "$OUT" "$PROFILE"; then
  # Success is an image on disk, not a zero exit status.
  IMAGE="$(ls -t "$OUT"/*.iso 2>/dev/null | head -1)"
  if [ -z "$IMAGE" ]; then
    warn "mkarchiso reported success and produced no image"
    say  "   the working directory is at $WORK/mkarchiso"
    exit 1
  fi

  step "Done"
  say ""
  ls -lh "$IMAGE" | sed 's/^/   /'
  say ""
  say "   Write it to a USB stick with:"
  say ""
  say "     sudo dd if=$IMAGE of=/dev/sdX bs=4M status=progress oflag=sync"
  say ""
  say "   /dev/sdX is the stick, not a partition on it, and everything on it goes."
  exit 0
fi

warn "mkarchiso failed"
say  "   the log is above, and the working directory is at $WORK"
exit 1
