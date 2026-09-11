#!/usr/bin/env bash
#
# Graphics drivers and CUDA.
#
# The hardware step already decided which NVIDIA branch this machine needs and
# wrote it down. This installs it and pins it, rather than deciding again: two
# places deciding the same thing is how they come to disagree.
#
# 580 is the branch that matters for the reference machine. It is the last one
# that supports Pascal; 590 dropped it. A machine with a GTX 1080 that takes a
# 590 update loses its display, which is why the pin is not optional.

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/lib.sh"

xstep "Graphics drivers"

BRANCH="$(resolved_nvidia_branch 2>/dev/null || true)"
if [ -z "$BRANCH" ]; then
  xlog "   No NVIDIA branch was resolved, so there is nothing to pin here."
  xlog "   00-hardware.sh has already installed whatever this machine needs."
  exit 0
fi

xlog "   the hardware step resolved branch $BRANCH"

case "$BRANCH" in
  580)     DRIVER=nvidia-580xx-dkms; UTILS=nvidia-580xx-utils ;;
  470)     DRIVER=nvidia-470xx-dkms; UTILS=nvidia-470xx-utils ;;
  390)     DRIVER=nvidia-390xx-dkms; UTILS=nvidia-390xx-utils ;;
  current) DRIVER=nvidia-open-dkms;  UTILS=nvidia-utils ;;
  *)       xlog "   branch $BRANCH needs no proprietary packages"; exit 0 ;;
esac

pin_install "$DRIVER" || xsoft_fail "$DRIVER did not install; 00-hardware.sh has already fallen back"
pin_install "$UTILS"  || xsoft_fail "$UTILS did not install"

# Both halves, held together. A driver and a userspace from different versions
# do not load, so pinning one without the other is worse than pinning neither.
hold_package "$DRIVER"
hold_package "$UTILS"

xstep "CUDA"
# Compute 6.1 is Pascal, which is the reference machine. Newer cards are a
# superset, so nothing here excludes them.
if pin_install cuda; then
  layer_into "etc/xos/cuda.env" <<'ENV'
# Written by XOS.
# Compute 6.1 is Pascal, the reference card. Building for it produces something
# every newer NVIDIA card can also run.
CUDA_ARCH=61
CUDAToolkit_ROOT=/opt/cuda
ENV
else
  xsoft_fail "CUDA did not install; local inference will fall back to the CPU"
fi

xlog ""
xlog "   Branch $BRANCH is installed and held. An update cannot move it."
exit 0
