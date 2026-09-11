#!/usr/bin/env bash
#
# Tests for the XOS install medium.
#
# Building the image needs Arch and mkarchiso, so that is not what this does.
# What it does is check everything that can be wrong before you get that far —
# and on a profile, most of it can. A misspelled package name, a boot entry
# pointing at a kernel path the profile does not produce, a bootmode listed
# with no configuration behind it: each of those costs a full build to
# discover, and every one is visible from here.
#
#   ./iso/tests/iso.test.sh

set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ISO="$HERE/.."
REPO="$(cd "$ISO/.." && pwd)"

PASSED=0
FAILED=0
pass() { printf '  ok    %s\n' "$1"; PASSED=$((PASSED + 1)); }
fail() { printf '  FAIL  %s\n' "$1"; printf '        %s\n' "${2:-}"; FAILED=$((FAILED + 1)); }
check() { if [ "$3" = "0" ]; then pass "$1"; else fail "$1" "$2"; fi; }

# ---------------------------------------------------------------- the profile

printf '\nThe profile is shaped the way archiso expects\n'

for file in profiledef.sh packages.x86_64 pacman.conf build.sh; do
  check "$file is there" "archiso needs it" \
    "$([ -f "$ISO/$file" ] && echo 0 || echo 1)"
done

check "profiledef.sh parses" "syntax error" \
  "$(bash -n "$ISO/profiledef.sh" 2>/dev/null && echo 0 || echo 1)"
check "build.sh parses" "syntax error" \
  "$(bash -n "$ISO/build.sh" 2>/dev/null && echo 0 || echo 1)"

# The settings mkarchiso actually reads.
for setting in iso_name iso_label install_dir buildmodes bootmodes arch \
               pacman_conf airootfs_image_type file_permissions; do
  check "profiledef.sh sets $setting" "mkarchiso reads it" \
    "$(grep -qE "^${setting}=|^${setting}\(" "$ISO/profiledef.sh" && echo 0 || echo 1)"
done

check "it builds for x86_64" "wrong architecture" \
  "$(grep -q 'arch="x86_64"' "$ISO/profiledef.sh" && echo 0 || echo 1)"

# ---------------------------------------------------------------- both firmwares

printf '\nIt boots on UEFI and on legacy BIOS\n'

# The entire reason this profile exists rather than a line in the manual
# telling people to use the Arch ISO.
check "legacy BIOS is a boot mode" "pre-2012 machines could not boot it" \
  "$(grep -q 'bios.syslinux' "$ISO/profiledef.sh" && echo 0 || echo 1)"
check "UEFI is a boot mode" "modern machines could not boot it" \
  "$(grep -q 'uefi-x64' "$ISO/profiledef.sh" && echo 0 || echo 1)"

# A bootmode with no configuration behind it fails at build time.
if grep -q 'bios.syslinux' "$ISO/profiledef.sh"; then
  check "syslinux has a configuration" "the BIOS bootmode has nothing behind it" \
    "$([ -f "$ISO/syslinux/syslinux.cfg" ] && echo 0 || echo 1)"
  for part in archiso_head.cfg archiso_sys.cfg archiso_tail.cfg archiso_pxe.cfg; do
    check "syslinux/$part is there" "syslinux.cfg includes it" \
      "$([ -f "$ISO/syslinux/$part" ] && echo 0 || echo 1)"
  done
fi
if grep -q 'systemd-boot' "$ISO/profiledef.sh"; then
  check "systemd-boot has a loader.conf" "the UEFI bootmode has nothing behind it" \
    "$([ -f "$ISO/efiboot/loader/loader.conf" ] && echo 0 || echo 1)"
  check "systemd-boot has at least one entry" "it would boot to an empty menu" \
    "$(ls "$ISO/efiboot/loader/entries/"*.conf >/dev/null 2>&1 && echo 0 || echo 1)"
fi

# Every include named in syslinux.cfg must exist, or the menu dies at boot.
MISSING_INCLUDES=""
for cfg in "$ISO"/syslinux/*.cfg; do
  while read -r included; do
    base="$(basename "$included")"
    [ -f "$ISO/syslinux/$base" ] || MISSING_INCLUDES="$MISSING_INCLUDES $base"
  done < <(grep -hoE '^(INCLUDE|CONFIG) +\S+' "$cfg" 2>/dev/null | awk '{print $2}')
done
check "every syslinux include exists" "missing:$MISSING_INCLUDES" \
  "$([ -z "$MISSING_INCLUDES" ] && echo 0 || echo 1)"

# Boot entries must name the kernel by the path archiso actually produces.
BAD_KERNEL=""
for entry in "$ISO"/efiboot/loader/entries/*.conf "$ISO"/syslinux/archiso_sys.cfg; do
  [ -f "$entry" ] || continue
  if grep -qiE '^(linux|LINUX)' "$entry"; then
    grep -qE 'vmlinuz-linux' "$entry" || BAD_KERNEL="$BAD_KERNEL $(basename "$entry")"
  fi
done
check "every boot entry names the kernel archiso builds" "wrong in:$BAD_KERNEL" \
  "$([ -z "$BAD_KERNEL" ] && echo 0 || echo 1)"

# A machine whose graphics the kernel cannot drive must still be installable.
check "a basic-graphics entry is offered on UEFI" "a black screen would end the install" \
  "$(grep -rqi 'nomodeset' "$ISO/efiboot/loader/entries/" && echo 0 || echo 1)"
check "a basic-graphics entry is offered on BIOS" "a black screen would end the install" \
  "$(grep -qi 'nomodeset' "$ISO/syslinux/archiso_sys.cfg" && echo 0 || echo 1)"

# ---------------------------------------------------------------- packages

printf '\nThe package list is sane\n'

PACKAGES="$(grep -vE '^#|^$' "$ISO/packages.x86_64")"
check "it is not empty" "nothing would be installed" \
  "$([ -n "$PACKAGES" ] && echo 0 || echo 1)"
check "every line looks like a package name" "a malformed name fails the whole build" \
  "$(printf '%s\n' "$PACKAGES" | grep -qvE '^[a-z0-9][a-z0-9._+-]*$' && echo 1 || echo 0)"
check "no duplicates" "pacman would complain" \
  "$([ "$(printf '%s\n' "$PACKAGES" | sort | uniq -d | wc -l)" = "0" ] && echo 0 || echo 1)"

# Without these the medium cannot do its one job.
for package in base linux mkinitcpio-archiso arch-install-scripts syslinux; do
  check "it carries $package" "the medium could not install anything" \
    "$(printf '%s\n' "$PACKAGES" | grep -qx "$package" && echo 0 || echo 1)"
done
# base.sh calls these by name.
for package in gptfdisk parted dosfstools e2fsprogs cryptsetup grub; do
  check "it carries $package, which base.sh calls" "the install would stop" \
    "$(printf '%s\n' "$PACKAGES" | grep -qx "$package" && echo 0 || echo 1)"
done
# The install scripts shell out to these.
for package in git curl python pciutils; do
  check "it carries $package, which the install scripts use" "a step would degrade" \
    "$(printf '%s\n' "$PACKAGES" | grep -qx "$package" && echo 0 || echo 1)"
done
check "networking is on the medium" "it could not fetch a single package" \
  "$(printf '%s\n' "$PACKAGES" | grep -qx networkmanager && echo 0 || echo 1)"

# ---------------------------------------------------------------- what it says

printf '\nThe medium tells somebody what to do\n'

check "there is a message on login" "a blank prompt and no instructions" \
  "$([ -f "$ISO/airootfs/etc/motd" ] && echo 0 || echo 1)"
check "it names one command to type" "somebody would have to read the source" \
  "$(grep -q 'install-xos' "$ISO/airootfs/etc/motd" && echo 0 || echo 1)"
check "that command exists on the medium" "it would name something that is not there" \
  "$([ -f "$ISO/airootfs/usr/local/bin/install-xos" ] && echo 0 || echo 1)"
check "it points at the installer the repo actually has" "wrong path" \
  "$(grep -q 'install/live.sh' "$ISO/airootfs/usr/local/bin/install-xos" && echo 0 || echo 1)"
check "live.sh is in the repo" "the medium would point at nothing" \
  "$([ -f "$REPO/install/live.sh" ] && echo 0 || echo 1)"
check "nothing starts erasing disks on login" "a machine left on a USB stick would install itself" \
  "$(grep -qE '^\s*(exec\s+)?(/root/)?install-xos|live\.sh' "$ISO/airootfs/root/.zlogin" && echo 1 || echo 0)"

# ---------------------------------------------------------------- the build

printf '\nThe build script assembles what the installer needs\n'

check "it puts the XOS source on the medium" "the installer would not be there" \
  "$(grep -q 'airootfs/root/xos' "$ISO/build.sh" && echo 0 || echo 1)"
check "it builds xos and xosd" "driver resolution during install would be blind" \
  "$(grep -q 'cargo build --release' "$ISO/build.sh" && echo 0 || echo 1)"
check "it ships the hardware database beside them" "the daemon could not resolve a driver" \
  "$(grep -q 'hardware-db.json' "$ISO/build.sh" && echo 0 || echo 1)"
check "it bundles the wifi drivers" "an unsupported chip would have nothing to try" \
  "$(grep -q 'DKMS_PACKAGES' "$ISO/build.sh" && echo 0 || echo 1)"
check "it enables networking on the live system" "the medium would boot with no network" \
  "$(grep -q 'NetworkManager.service' "$ISO/build.sh" && echo 0 || echo 1)"
check "it refuses to run without mkarchiso" "it would fail confusingly, late" \
  "$(grep -q 'need mkarchiso' "$ISO/build.sh" && echo 0 || echo 1)"
check "--check reports without building" "no way to see what is missing first" \
  "$(grep -q 'CHECK_ONLY' "$ISO/build.sh" && echo 0 || echo 1)"

# The drivers the build script writes must be where 00-hardware.sh looks.
DKMS_BUILD_PATH="$(grep -oE 'DKMS_DIR="\$PROFILE/airootfs[^"]*"' "$ISO/build.sh" | head -1 | sed 's|.*airootfs||; s|"||')"
check "the bundled drivers land where 00-hardware.sh looks" \
  "build.sh writes $DKMS_BUILD_PATH and the hardware step would not find it" \
  "$(grep -q -- "$DKMS_BUILD_PATH" "$REPO/install/00-hardware.sh" && echo 0 || echo 1)"

# ---------------------------------------------------------------- the installer

printf '\nThe all-in-one installer\n'

check "live.sh parses" "syntax error" \
  "$(bash -n "$REPO/install/live.sh" 2>/dev/null && echo 0 || echo 1)"
check "it does not use set -e" "one failing command would abort an install" \
  "$(grep -qE '^set -[a-z]*e[a-z]*[[:space:]]*$' "$REPO/install/live.sh" && echo 1 || echo 0)"
check "it starts a daemon so drivers resolve" "the hardware step would find no inventory" \
  "$(grep -q 'start_daemon' "$REPO/install/live.sh" && echo 0 || echo 1)"
check "it stops the daemon again" "it would be left running on the live system" \
  "$(grep -q 'stop_daemon' "$REPO/install/live.sh" && echo 0 || echo 1)"
check "it runs base.sh and then install.sh, in that order" "the layer needs a system under it" \
  "$([ "$(grep -n 'base.sh' "$REPO/install/live.sh" | tail -1 | cut -d: -f1)" -lt \
      "$(grep -n 'install.sh' "$REPO/install/live.sh" | tail -1 | cut -d: -f1)" ] && echo 0 || echo 1)"
check "it will not choose a disk unattended" "it could erase the wrong one" \
  "$(grep -q 'nothing is reading the prompt' "$REPO/install/live.sh" && echo 0 || echo 1)"
check "it offers a dry run" "no way to see the plan first" \
  "$(grep -q 'dry-run' "$REPO/install/live.sh" && echo 0 || echo 1)"
# The behaviour, not a comment about it. One destructive confirmation, asked
# once, in the file that does the destroying. Two prompts for one action teach
# people to click through both.
check "base.sh asks for the disk name to be typed back" "nothing confirms the erase" \
  "$(grep -q 'Type the disk name' "$REPO/install/base.sh" && echo 0 || echo 1)"
check "live.sh does not ask a second time" "two prompts teach people to click through both" \
  "$(grep -q 'Type the disk name' "$REPO/install/live.sh" && echo 1 || echo 0)"

printf '\n%s passed, %s failed\n\n' "$PASSED" "$FAILED"
[ "$FAILED" = "0" ]
