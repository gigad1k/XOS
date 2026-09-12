#!/usr/bin/env bash
# shellcheck disable=SC2034
#
# The XOS install medium.
#
# An archiso profile. `mkarchiso` reads this, builds a live system from
# packages.x86_64, lays airootfs/ over it, and writes a hybrid ISO that boots
# on both UEFI and legacy BIOS.
#
# Both matter. Omarchy's own installer requires UEFI, which would exclude most
# machines made before about 2012 — and those machines are not an edge case for
# XOS, they are the target. The bootmodes below are why this profile exists
# rather than a line in the manual telling people to use the Arch ISO.

iso_name="xos"
iso_label="XOS_$(date +%Y%m)"
iso_publisher="XOS <https://github.com/gigad1k/XOS>"
iso_application="XOS install medium"
iso_version="$(date +%Y.%m.%d)"
install_dir="arch"
buildmodes=('iso')

# BIOS first in the list, deliberately. It is the mode most likely to be
# forgotten and the one this project cannot do without.
#
# These two names are the current spelling. archiso once wanted the mode named
# per-medium — `bios.syslinux.mbr` and `bios.syslinux.eltorito`, `uefi-x64.…esp`
# and `…eltorito` — and still accepts those, with a deprecation warning, by
# rewriting them to exactly these. Writing the old names would build the same
# image and tell whoever built it off, four times, for no reason.
bootmodes=(
  'bios.syslinux'
  'uefi.systemd-boot'
)

arch="x86_64"
pacman_conf="pacman.conf"
airootfs_image_type="squashfs"

# xz with the x86 filter. Slower to build and meaningfully smaller to download,
# which is the right trade for something people fetch once over a connection
# that may not be quick.
airootfs_image_tool_options=('-comp' 'xz' '-Xbcj' 'x86' '-b' '1M' '-Xdict-size' '1M')

bootstrap_tarball_compression=(zstd -c -T0 --auto-threads=logical --long -19)

# mkarchiso does not preserve the modes of anything in airootfs: files arrive
# as 644 and directories as 755, whoever owned them and whatever they were
# committed as. Only the paths listed here get the mode they need.
#
# /usr/local/bin/install-xos was absent from this list once, so the one command
# the medium tells people to type shipped non-executable. The medium booted
# perfectly and could not install anything.
file_permissions=(
  ["/etc/shadow"]="0:0:400"
  ["/etc/gshadow"]="0:0:400"
  ["/root"]="0:0:750"
  ["/root/.bash_profile"]="0:0:644"
  ["/root/install-xos"]="0:0:755"
  ["/usr/local/bin/install-xos"]="0:0:755"
  ["/usr/local/bin/xos"]="0:0:755"
  ["/usr/local/bin/xosd"]="0:0:755"
)
