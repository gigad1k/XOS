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
#
# Read from the array rather than by grepping the file. A comment that merely
# mentions a boot mode is not a boot mode, and a check that cannot tell the
# difference passes while the profile says something else.
BOOTMODES="$(sed -n '/^bootmodes=(/,/^)/p' "$ISO/profiledef.sh" | grep -oE "'[a-z0-9._-]+'" | tr -d "'")"

check "the profile declares boot modes at all" "mkarchiso would have nothing to build" \
  "$([ -n "$BOOTMODES" ] && echo 0 || echo 1)"
check "legacy BIOS is a boot mode" "pre-2012 machines could not boot it" \
  "$(printf '%s\n' "$BOOTMODES" | grep -q '^bios\.' && echo 0 || echo 1)"
check "UEFI is a boot mode" "modern machines could not boot it" \
  "$(printf '%s\n' "$BOOTMODES" | grep -q '^uefi' && echo 0 || echo 1)"

# archiso accepts the older per-medium spellings and warns four times while
# rewriting them to exactly what is written here now.
DEPRECATED="$(printf '%s\n' "$BOOTMODES" | grep -E '\.(mbr|eltorito|esp)$|^uefi-(x64|ia32)\.' || true)"
check "no boot mode uses a deprecated spelling" "archiso warns: $DEPRECATED" \
  "$([ -z "$DEPRECATED" ] && echo 0 || echo 1)"

# A bootmode with no configuration behind it fails at build time.
if printf '%s\n' "$BOOTMODES" | grep -q '^bios\.'; then
  check "syslinux has a configuration" "the BIOS bootmode has nothing behind it" \
    "$([ -f "$ISO/syslinux/syslinux.cfg" ] && echo 0 || echo 1)"
  for part in archiso_head.cfg archiso_sys.cfg archiso_tail.cfg; do
    check "syslinux/$part is there" "syslinux.cfg includes it" \
      "$([ -f "$ISO/syslinux/$part" ] && echo 0 || echo 1)"
  done
fi
if printf '%s\n' "$BOOTMODES" | grep -q 'systemd-boot'; then
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

# ---------------------------------------------------------------- resolution
#
# Every one of these was written wrong, the profile built cleanly, 73 tests
# passed, and the machine stopped at a bare boot: prompt. ISOLINUX resolves a
# relative path against the directory holding isolinux.bin, which archiso puts
# at /boot/syslinux — so "boot/syslinux/whichsys.c32" asks for
# /boot/syslinux/boot/syslinux/whichsys.c32 and finds nothing.

printf '\nsyslinux paths resolve where syslinux looks\n'

PREFIXED="$(grep -hnE '^[[:space:]]*(COM32|CONFIG|INCLUDE|UI|MENU BACKGROUND)[[:space:]]+boot/' \
  "$ISO"/syslinux/*.cfg 2>/dev/null || true)"
check "no module or config is named with a directory prefix" "would resolve under /boot/syslinux/: $PREFIXED" \
  "$([ -z "$PREFIXED" ] && echo 0 || echo 1)"

# The kernel is the exception that proves it: it lives outside that directory,
# so it needs a leading slash and the install directory.
BAD_LINUX="$(grep -hnE '^[[:space:]]*(LINUX|INITRD)[[:space:]]+[^/]' "$ISO"/syslinux/*.cfg 2>/dev/null || true)"
check "every kernel and initramfs path is absolute" "relative, so it resolves under /boot/syslinux/: $BAD_LINUX" \
  "$([ -z "$BAD_LINUX" ] && echo 0 || echo 1)"
check "the kernel path uses the install directory placeholder" "it would point outside the medium" \
  "$(grep -q 'LINUX /%INSTALL_DIR%/' "$ISO/syslinux/archiso_sys.cfg" && echo 0 || echo 1)"

# Every file the menu names has to travel with it.
MISSING_ASSETS=""
for asset in $(grep -hoE '^[[:space:]]*(UI|MENU BACKGROUND)[[:space:]]+\S+' "$ISO"/syslinux/*.cfg \
               | awk '{print $NF}' | sort -u); do
  case "$asset" in
    *.c32) continue ;;   # archiso copies every .c32 from the syslinux package
  esac
  [ -f "$ISO/syslinux/$asset" ] || MISSING_ASSETS="$MISSING_ASSETS $asset"
done
check "every asset the menu names is in the profile" "missing:$MISSING_ASSETS" \
  "$([ -z "$MISSING_ASSETS" ] && echo 0 || echo 1)"

# A menu with no default waits for a keypress that never comes on a machine
# left to install itself.
check "the BIOS menu has a default entry" "it would wait forever" \
  "$(grep -qE '^DEFAULT ' "$ISO/syslinux/archiso_sys.cfg" && echo 0 || echo 1)"
check "the BIOS menu has a timeout" "it would wait forever" \
  "$(grep -qE '^TIMEOUT ' "$ISO/syslinux/archiso_sys.cfg" && echo 0 || echo 1)"

# Both firmwares should find the medium the same way. Two answers to one
# question is one too many, and label matching can pick the wrong disk.
check "BIOS finds the medium by UUID" "a colliding label could select the wrong disk" \
  "$(grep -q 'archisosearchuuid=%ARCHISO_UUID%' "$ISO/syslinux/archiso_sys.cfg" && echo 0 || echo 1)"
check "UEFI finds the medium the same way" "the two firmwares would disagree" \
  "$(grep -rq 'archisosearchuuid=%ARCHISO_UUID%' "$ISO/efiboot/loader/entries/" && echo 0 || echo 1)"

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

# ---------------------------------------------------------------- the boot
#
# Every one of these was wrong at once, the image built, 81 tests passed, and
# the machine reached an emergency shell with the root account locked. A
# bootloader menu proves the bootloader; it proves nothing about the boot.

printf '
The initramfs can find the medium
'

ARCHISO_CONF="$ISO/airootfs/etc/mkinitcpio.conf.d/archiso.conf"
PRESET="$ISO/airootfs/etc/mkinitcpio.d/linux.preset"

check "the profile configures the archiso hooks" "the initramfs gets the stock hooks and cannot mount the medium"   "$([ -f "$ARCHISO_CONF" ] && echo 0 || echo 1)"
check "the archiso hook is among them" "switch root fails and it drops to emergency mode"   "$(grep -qE '^HOOKS=\(.*[( ]archiso[ )]' "$ARCHISO_CONF" 2>/dev/null && echo 0 || echo 1)"
check "autodetect is not among them" "it would probe the build machine, not the target"   "$(grep -qE '^HOOKS=\(.*[( ]autodetect[ )]' "$ARCHISO_CONF" 2>/dev/null && echo 1 || echo 0)"
check "a preset names the image the boot entries load" "the bootloader would point at nothing"   "$(grep -q 'initramfs-linux.img' "$PRESET" 2>/dev/null && echo 0 || echo 1)"

# Every PXE hook needs a package, and missing one stops mkinitcpio dead.
HOOK_PKGS=""
grep -qE 'archiso_pxe_(common|nfs)' "$ARCHISO_CONF" 2>/dev/null &&
  { grep -qx 'mkinitcpio-nfs-utils' "$ISO/packages.x86_64" || HOOK_PKGS="$HOOK_PKGS mkinitcpio-nfs-utils"; }
grep -q 'archiso_pxe_nbd' "$ARCHISO_CONF" 2>/dev/null &&
  { grep -qx 'nbd' "$ISO/packages.x86_64" || HOOK_PKGS="$HOOK_PKGS nbd"; }
check "every hook has the package it needs" "mkinitcpio would fail on:$HOOK_PKGS"   "$([ -z "$HOOK_PKGS" ] && echo 0 || echo 1)"

printf '
Somebody can actually use the medium
'

# mkarchiso discards airootfs modes. Anything that must be executable has to be
# named in file_permissions, or it ships 644 and cannot run.
NEEDS_MODE="$(grep -lE '^#!' "$ISO"/airootfs/usr/local/bin/* 2>/dev/null | xargs -rn1 basename)"
UNLISTED=""
for script in $NEEDS_MODE; do
  grep -q "\[\"/usr/local/bin/$script\"\]" "$ISO/profiledef.sh" || UNLISTED="$UNLISTED $script"
done
check "every script on PATH is made executable" "would ship 644 and refuse to run:$UNLISTED"   "$([ -z "$UNLISTED" ] && echo 0 || echo 1)"

ZLOGIN_WITHOUT_ZSH=0
[ -f "$ISO/airootfs/root/.zlogin" ] && ! grep -qx 'zsh' "$ISO/packages.x86_64" && ZLOGIN_WITHOUT_ZSH=1
check "no login file is written for a shell the medium does not carry" "a .zlogin without zsh never runs" \
  "$ZLOGIN_WITHOUT_ZSH"

check "somebody can reach a shell to type it" "the root account would be locked"   "$([ -f "$ISO/airootfs/etc/systemd/system/getty@tty1.service.d/autologin.conf" ] && echo 0 || echo 1)"

# The installer is reached through bash, so it never depends on a mode that
# mkarchiso throws away.
check "the launcher does not depend on a discarded mode" "live.sh ships 644 and exec would fail"   "$(grep -q 'exec bash' "$ISO/airootfs/usr/local/bin/install-xos" && echo 0 || echo 1)"

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
# The medium starts the installer by itself, so nobody has to be told a
# command. What it must never do is erase a disk without being asked, and that
# is a different property: the unattended path exists, it is reachable only
# from a boot menu entry that says what it does, and it counts down first.
LOGIN_FILE="$ISO/airootfs/root/.bash_profile"

check "the medium starts the installer by itself" "somebody would have to know a command to type" \
  "$(grep -q 'install-xos' "$LOGIN_FILE" && echo 0 || echo 1)"
check "the login file is one the shell on the medium actually reads" "it would never run" \
  "$([ -f "$ISO/airootfs/root/.bash_profile" ] && echo 0 || echo 1)"
check "an unattended install is only reached deliberately" "a forgotten USB stick would erase the machine" \
  "$(grep -q 'xos.auto' "$LOGIN_FILE" && echo 0 || echo 1)"
check "the unattended entry says it erases the disk" "it would not be an informed choice" \
  "$(grep -qi 'erase' "$ISO/syslinux/archiso_sys.cfg" && echo 0 || echo 1)"
check "the unattended path counts down before it starts" "no chance to stop it" \
  "$(grep -q 'Ctrl+C now' "$REPO/install/live.sh" && echo 0 || echo 1)"
check "the default path still asks which disk" "it would pick one on its own" \
  "$(grep -q 'Which disk?' "$REPO/install/live.sh" && echo 0 || echo 1)"

# agetty respawns the login shell when it exits, and this file IS that shell.
# An installer started here that exits for any reason is therefore started
# again, immediately, with nothing bounding it. On the unattended entry that is
# a machine re-erasing a disk every couple of minutes with nobody watching, and
# it is exactly what a real boot did before these four checks existed.
check "the installer is not replaced by exec, so a shell outlives it" "a failed install would leave no way to read the log" \
  "$(grep -qE '^[[:space:]]*exec[[:space:]]+install-xos' "$LOGIN_FILE" && echo 1 || echo 0)"
check "it starts the installer once per boot, not once per login" "an installer that exits would restart forever" \
  "$(grep -q '/run/xos-installer-started' "$LOGIN_FILE" && echo 0 || echo 1)"
check "the marker lives in tmpfs so a reboot starts over" "a stale marker would stop the medium installing at all" \
  "$(grep -q ': > /run/xos-installer-started' "$LOGIN_FILE" && echo 0 || echo 1)"
check "a second login says why it did not start" "it would look broken" \
  "$(grep -q 'already ran during this boot' "$LOGIN_FILE" && echo 0 || echo 1)"

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
# The daemon told the installer nothing for fifteen seconds and the installer
# concluded it had failed to start. It had not: xosd listened on its configured
# path while XOS_SOCKET, which only the client read, pointed somewhere else.
check "the daemon is asked to listen where the client will look" "the two ends would point at different paths" \
  "$(grep -q 'export XOS_SOCKET' "$REPO/install/live.sh" && echo 0 || echo 1)"
check "and xosd actually reads that variable" "setting it would move only the client" \
  "$(grep -q 'XOS_SOCKET' "$REPO/xosd/src/config.rs" && echo 0 || echo 1)"
check "asking the daemon for hardware cannot hang the install" "an unattended install would sit there forever" \
  "$(grep -q 'timeout 30 "$XOS_BIN" hardware --json' "$REPO/install/live.sh" && echo 0 || echo 1)"
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
