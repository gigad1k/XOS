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
say "   NetworkManager and sshd will start on the live system"

# ---------------------------------------------------------------- XOS itself

step "Putting XOS on the medium"

SOURCE="$PROFILE/airootfs/root/xos"
mkdir -p "$SOURCE"
# The working tree as committed, so the medium carries no build output and no
# half-finished edit.
if git -C "$REPO" rev-parse --git-dir >/dev/null 2>&1; then
  git -C "$REPO" archive --format=tar HEAD | tar -x -C "$SOURCE"
  say "   source from git HEAD: $(git -C "$REPO" rev-parse --short HEAD)"
else
  rsync -a --exclude target --exclude .git "$REPO/" "$SOURCE/"
  say "   source copied from the working tree"
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

  BUILDER="${SUDO_USER:-nobody}"
  BUILD_ROOT="$WORK/dkms"
  rm -rf "$BUILD_ROOT"; mkdir -p "$BUILD_ROOT"
  chown "$BUILDER" "$BUILD_ROOT" 2>/dev/null

  BUILT=0
  for package in "${DKMS_PACKAGES[@]}"; do
    say ""
    say "   $package"
    if ! sudo -u "$BUILDER" git clone --depth 1 \
           "https://aur.archlinux.org/$package.git" "$BUILD_ROOT/$package" >/dev/null 2>&1; then
      warn "   could not fetch $package; skipping it"
      continue
    fi
    # makepkg refuses to run as root, correctly.
    if ( cd "$BUILD_ROOT/$package" && sudo -u "$BUILDER" makepkg -s --noconfirm --needed ) >/dev/null 2>&1; then
      cp "$BUILD_ROOT/$package"/*.pkg.tar.zst "$DKMS_DIR/" 2>/dev/null && BUILT=$((BUILT + 1))
      say "   built"
    else
      warn "   $package did not build; skipping it"
    fi
  done

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

if mkarchiso -v -w "$WORK/mkarchiso" -o "$OUT" "$PROFILE"; then
  step "Done"
  say ""
  ls -lh "$OUT"/*.iso 2>/dev/null | sed 's/^/   /'
  say ""
  say "   Write it to a USB stick with:"
  say ""
  say "     sudo dd if=$OUT/xos-*.iso of=/dev/sdX bs=4M status=progress oflag=sync"
  say ""
  say "   /dev/sdX is the stick, not a partition on it, and everything on it goes."
  exit 0
fi

warn "mkarchiso failed"
say  "   the log is above, and the working directory is at $WORK"
exit 1
