#!/usr/bin/env bash
#
# The XOS base installer: partition a disk, put Arch on it, make it boot.
#
# XOS ships this rather than using Omarchy's ISO for one reason. Omarchy's
# installer requires UEFI, and most machines from before about 2012 boot legacy
# BIOS. Those machines are not an edge case for XOS, they are the target: the
# whole project exists so that a decade-old PC becomes useful again. An
# installer that refuses them defeats the point.
#
# # This is the only file in XOS that destroys data
#
# Everything else layers, falls back, and can be undone. This writes partition
# tables. So it asks first, in plain words, showing exactly which disk and what
# will happen to it, and it does nothing at all under --dry-run.
#
#   ./base.sh --dry-run          plan it, touch nothing
#   ./base.sh --disk /dev/sda    do it, after confirming
#   ./base.sh --disk /dev/sda --encrypt
#
# After this, install.sh applies the XOS layer.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$HERE/lib.sh"

DISK=""
ENCRYPT=0
ASSUME_YES=0
HOSTNAME_WANTED="xos"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --disk) DISK="${2:-}"; shift 2 ;;
    --encrypt) ENCRYPT=1; shift ;;
    --hostname) HOSTNAME_WANTED="${2:-xos}"; shift 2 ;;
    --yes) ASSUME_YES=1; shift ;;
    --dry-run) XOS_DRY_RUN=1; shift ;;
    *) shift ;;
  esac
done

MOUNT="${XOS_MOUNT:-/mnt}"

# ---------------------------------------------------------------- firmware

xstep "Firmware"

FIRMWARE="$(firmware_mode)"
case "$FIRMWARE" in
  uefi)
    xlog "   UEFI. The disk gets a GPT label and an EFI system partition."
    ;;
  bios)
    xlog "   Legacy BIOS. The disk gets an MBR label and a BIOS boot partition."
    xlog "   This is the case Omarchy's own installer cannot do, and the reason"
    xlog "   XOS ships this file."
    ;;
  *)
    # Guessing here means an unbootable machine, and the person finds out after
    # the install rather than before it.
    xwarn "The firmware mode cannot be determined on this machine."
    xlog "   Partitioning needs a definite answer, and guessing produces a"
    xlog "   machine that installs cleanly and then will not boot."
    xlog "   Run this on the real hardware, or pass XOS_FORCE_FIRMWARE=uefi|bios"
    xlog "   if you are certain."
    FIRMWARE="${XOS_FORCE_FIRMWARE:-}"
    if [ -z "$FIRMWARE" ]; then
      exit 1
    fi
    xlog "   proceeding as $FIRMWARE because you said so"
    ;;
esac

# ---------------------------------------------------------------- encryption

xstep "Encryption"

if [ "$ENCRYPT" = "1" ]; then
  if has_aes_ni; then
    xlog "   This CPU has AES-NI, so encryption costs almost nothing. Good choice."
  else
    # Opt-in rather than mandatory, and this is why. On a pre-AES-NI CPU,
    # full-disk encryption turns an already slow machine into an unusable one,
    # and many of the machines XOS targets predate it.
    xwarn "This CPU has no AES-NI."
    xlog "   Full-disk encryption on it is done in software and will make the"
    xlog "   machine noticeably slower at everything that touches the disk."
    xlog "   XOS will still do it, because it is your machine, but on hardware"
    xlog "   this old the honest recommendation is to leave it off."
    if [ "$ASSUME_YES" != "1" ] && [ -t 0 ]; then
      printf '   Encrypt anyway? [y/N] '
      read -r answer
      case "$answer" in
        [Yy]*) ;;
        *) ENCRYPT=0; xlog "   not encrypting" ;;
      esac
    fi
  fi
else
  if has_aes_ni; then
    xlog "   Not encrypting. This CPU has AES-NI, so --encrypt would be close to free."
  else
    xlog "   Not encrypting, which on a CPU without AES-NI is the sensible default."
  fi
fi

# ---------------------------------------------------------------- the disk

xstep "The disk"

if [ -z "$DISK" ]; then
  xlog "   Disks on this machine:"
  lsblk -dno NAME,SIZE,MODEL 2>/dev/null | sed 's/^/     /'
  xwarn "No disk chosen. Pass --disk /dev/sdX."
  xlog "   Nothing has been changed."
  exit 1
fi

if [ ! -b "$DISK" ] && [ "$XOS_DRY_RUN" != "1" ]; then
  xwarn "$DISK is not a block device. Nothing has been changed."
  exit 1
fi

SIZE="$(lsblk -dno SIZE "$DISK" 2>/dev/null | tr -d ' ')"
MODEL="$(lsblk -dno MODEL "$DISK" 2>/dev/null | sed 's/[[:space:]]*$//')"
xlog "   $DISK  ${SIZE:-unknown size}  ${MODEL:-unknown model}"
xlog ""
xlog "   What is on it now:"
lsblk -no NAME,SIZE,FSTYPE,LABEL,MOUNTPOINT "$DISK" 2>/dev/null | sed 's/^/     /' \
  || xlog "     (cannot read it)"

xlog ""
xlog "   ####################################################################"
xlog "   EVERYTHING ON $DISK WILL BE DESTROYED."
xlog "   That includes any other operating system, and every file on it."
xlog "   This cannot be undone, and XOS has no copy of any of it."
xlog "   ####################################################################"

if [ "$XOS_DRY_RUN" = "1" ]; then
  xlog ""
  xlog "   This is a dry run. Nothing will be written."
elif [ "$ASSUME_YES" != "1" ]; then
  if [ ! -t 0 ]; then
    # A destructive step must never proceed because nobody was there to say no.
    xwarn "Nothing is reading the prompt, so nothing will be erased."
    xlog "   Run this where someone can answer, or pass --yes if you are sure."
    exit 1
  fi
  printf '   Type the disk name to confirm (%s): ' "$DISK"
  read -r typed
  if [ "$typed" != "$DISK" ]; then
    xlog "   That did not match. Nothing has been changed."
    exit 1
  fi
fi

# ---------------------------------------------------------------- partitions

xstep "Partitioning"

run() {
  if [ "$XOS_DRY_RUN" = "1" ]; then
    xlog "   would run: $*"
    return 0
  fi
  xlog "   $*"
  "$@" >> "$XOS_LOG" 2>&1
}

# Partition names differ between /dev/sda1 and /dev/nvme0n1p1.
part() {
  case "$DISK" in
    *nvme*|*mmcblk*) printf '%sp%s' "$DISK" "$1" ;;
    *) printf '%s%s' "$DISK" "$1" ;;
  esac
}

if [ "$FIRMWARE" = "uefi" ]; then
  # GPT, an EFI system partition, and root.
  run sgdisk --zap-all "$DISK"
  run sgdisk --new=1:0:+512M --typecode=1:ef00 --change-name=1:XOSBOOT "$DISK"
  run sgdisk --new=2:0:0     --typecode=2:8300 --change-name=2:XOSROOT "$DISK"
  BOOT_PART="$(part 1)"
  ROOT_PART="$(part 2)"
else
  # MBR, with a small BIOS boot partition for GRUB's core image. Without it,
  # GRUB has nowhere to put the part of itself that does not fit in the MBR gap,
  # and the install finishes and then does not boot.
  run parted -s "$DISK" mklabel msdos
  run parted -s "$DISK" mkpart primary ext4 1MiB 512MiB
  run parted -s "$DISK" set 1 boot on
  run parted -s "$DISK" mkpart primary ext4 512MiB 100%
  BOOT_PART="$(part 1)"
  ROOT_PART="$(part 2)"
fi

xlog "   boot: $BOOT_PART"
xlog "   root: $ROOT_PART"

# ---------------------------------------------------------------- filesystems

xstep "Filesystems"

if [ "$ENCRYPT" = "1" ]; then
  run cryptsetup luksFormat --type luks2 "$ROOT_PART"
  run cryptsetup open "$ROOT_PART" xosroot
  ROOT_DEVICE="/dev/mapper/xosroot"
else
  ROOT_DEVICE="$ROOT_PART"
fi

if [ "$FIRMWARE" = "uefi" ]; then
  run mkfs.fat -F32 "$BOOT_PART"
else
  run mkfs.ext4 -F "$BOOT_PART"
fi
run mkfs.ext4 -F "$ROOT_DEVICE"

run mkdir -p "$MOUNT"
run mount "$ROOT_DEVICE" "$MOUNT"
run mkdir -p "$MOUNT/boot"
run mount "$BOOT_PART" "$MOUNT/boot"

# ---------------------------------------------------------------- base Arch

xstep "Arch"

BASE_PACKAGES=(base base-devel linux linux-firmware sof-firmware networkmanager sudo)
[ "$ENCRYPT" = "1" ] && BASE_PACKAGES+=(cryptsetup)
[ "$FIRMWARE" = "uefi" ] && BASE_PACKAGES+=(efibootmgr)
BASE_PACKAGES+=(grub dosfstools e2fsprogs)

for package in "${BASE_PACKAGES[@]}"; do
  if ! pinned_version "$package" >/dev/null 2>&1; then
    xwarn "$package is not pinned in packages.lock"
  fi
done

run pacstrap -K "$MOUNT" "${BASE_PACKAGES[@]}"

if [ "$XOS_DRY_RUN" = "1" ]; then
  xlog "   would write $MOUNT/etc/fstab"
else
  genfstab -U "$MOUNT" >> "$MOUNT/etc/fstab" 2>> "$XOS_LOG" \
    || xsoft_fail "could not write fstab"
  printf '%s\n' "$HOSTNAME_WANTED" > "$MOUNT/etc/hostname" 2>/dev/null
fi

# ---------------------------------------------------------------- the boot

xstep "Making it boot"

# Whatever the hardware step decided the kernel needs, from a file it wrote
# rather than from anything guessed here.
KERNEL_EXTRA=""
if [ -f "$XOS_ROOT/etc/xos/kernel-parameters" ]; then
  KERNEL_EXTRA="$(tr '\n' ' ' < "$XOS_ROOT/etc/xos/kernel-parameters")"
  xlog "   kernel parameters from the hardware step: $KERNEL_EXTRA"
fi

if [ "$FIRMWARE" = "uefi" ]; then
  run arch-chroot "$MOUNT" grub-install --target=x86_64-efi \
    --efi-directory=/boot --bootloader-id=XOS
else
  # GRUB to the MBR. This is the path Omarchy's installer does not have.
  run arch-chroot "$MOUNT" grub-install --target=i386-pc "$DISK"
fi

if [ "$XOS_DRY_RUN" = "1" ]; then
  xlog "   would set the kernel command line and write the GRUB config"
else
  if [ -n "$KERNEL_EXTRA" ] && [ -f "$MOUNT/etc/default/grub" ]; then
    sed -i "s|^GRUB_CMDLINE_LINUX_DEFAULT=\"\(.*\)\"|GRUB_CMDLINE_LINUX_DEFAULT=\"\1 $KERNEL_EXTRA\"|" \
      "$MOUNT/etc/default/grub" 2>/dev/null
  fi
  arch-chroot "$MOUNT" grub-mkconfig -o /boot/grub/grub.cfg >> "$XOS_LOG" 2>&1 \
    || xsoft_fail "grub-mkconfig failed; the machine may not boot"
fi

run arch-chroot "$MOUNT" systemctl enable NetworkManager

xstep "Base install finished"
xlog "   Arch is on $DISK and it boots $FIRMWARE."
xlog "   Next: install.sh applies the XOS layer."
exit 0
