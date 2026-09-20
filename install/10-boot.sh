#!/usr/bin/env bash
#
# The kernel command line, once the hardware step has decided what it needs.
#
# # Why this is a step of its own, and why it is last
#
# base.sh installs GRUB and generates its config while it is laying down the
# base system. The hardware step runs afterwards, inside the XOS layer, and that
# is where this machine's card is resolved and where any kernel parameter it
# needs gets written. By then grub.cfg has already been generated from a command
# line that knew nothing about them.
#
# So the parameters were written to /etc/xos/kernel-parameters and nothing ever
# read them back. On the hardware XOS is aimed at that is not cosmetic: a card
# whose resolution asks for nvidia-drm.modeset=1 and does not get it comes up to
# a black screen, which is the single outcome the hardware step exists to
# prevent.
#
# Hence: the last step, after everything that could decide it needs a parameter,
# and the only place that regenerates the boot config with the answer.
#
# # What it will not do
#
# It does not invent parameters, and it does not remove anything already on the
# command line. It adds what the hardware step asked for, and only what is not
# there already, so running the layer twice is not a way to accumulate the same
# parameter four times.
#
# It is never fatal. A machine that boots with the wrong parameters is
# recoverable from the boot menu; a machine whose install aborted halfway is not.

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$HERE/lib.sh"

PARAMETERS_FILE="$XOS_ROOT/etc/xos/kernel-parameters"
GRUB_DEFAULTS="$XOS_ROOT/etc/default/grub"
GRUB_CONFIG="/boot/grub/grub.cfg"

xstep "Kernel command line"

if [ ! -f "$PARAMETERS_FILE" ]; then
  xlog "   the hardware step asked for no kernel parameters"
  exit 0
fi

WANTED="$(tr '\n' ' ' < "$PARAMETERS_FILE" 2>/dev/null | tr -s ' ')"
WANTED="${WANTED# }"
WANTED="${WANTED% }"

if [ -z "$WANTED" ]; then
  xlog "   the hardware step asked for no kernel parameters"
  exit 0
fi

xlog "   the hardware step asked for: $WANTED"

if [ ! -f "$GRUB_DEFAULTS" ]; then
  # Another bootloader, or none yet. Saying so is the point: somebody reading
  # the log needs to know these did not get applied, rather than assuming.
  xwarn "there is no GRUB configuration on this machine, so these were not applied"
  xlog "   If it boots some other way, add them to its command line by hand:"
  xlog "     $WANTED"
  exit 0
fi

CURRENT="$(grep -m1 '^GRUB_CMDLINE_LINUX_DEFAULT=' "$GRUB_DEFAULTS" 2>/dev/null \
  | sed 's/^GRUB_CMDLINE_LINUX_DEFAULT="\(.*\)"$/\1/')"

MISSING=()
for parameter in $WANTED; do
  case " $CURRENT " in
    *" $parameter "*) continue ;;
  esac
  MISSING+=("$parameter")
done

if [ "${#MISSING[@]}" = "0" ]; then
  xlog "   already on the command line; nothing to change"
  exit 0
fi

xlog "   adding: ${MISSING[*]}"

if [ "$XOS_DRY_RUN" = "1" ]; then
  xlog "   would add them to $GRUB_DEFAULTS and regenerate $GRUB_CONFIG"
  exit 0
fi

UPDATED="$CURRENT ${MISSING[*]}"
UPDATED="${UPDATED# }"

if grep -q '^GRUB_CMDLINE_LINUX_DEFAULT=' "$GRUB_DEFAULTS" 2>/dev/null; then
  sed -i "s|^GRUB_CMDLINE_LINUX_DEFAULT=.*|GRUB_CMDLINE_LINUX_DEFAULT=\"$UPDATED\"|" \
    "$GRUB_DEFAULTS" 2>/dev/null
else
  printf '\n# Added by XOS: the hardware step resolved these for this machine.\nGRUB_CMDLINE_LINUX_DEFAULT="%s"\n' \
    "$UPDATED" >> "$GRUB_DEFAULTS" 2>/dev/null
fi

# Check it landed. A sed whose pattern did not match changes nothing and reports
# nothing, and the machine then boots without the parameter it needed while the
# log says everything went fine.
for parameter in "${MISSING[@]}"; do
  if ! grep -q -- "$parameter" "$GRUB_DEFAULTS" 2>/dev/null; then
    xsoft_fail "could not put $parameter on the command line; this machine may come up wrong"
    exit 0
  fi
done

# And regenerate, because /etc/default/grub is an input to grub-mkconfig and
# nothing else. Editing it without this step is the bug this file was written
# to fix, one level up.
if [ -n "$XOS_ROOT" ] && command -v arch-chroot >/dev/null 2>&1; then
  REGENERATED="arch-chroot $XOS_ROOT grub-mkconfig -o $GRUB_CONFIG"
  arch-chroot "$XOS_ROOT" grub-mkconfig -o "$GRUB_CONFIG" >> "$XOS_LOG" 2>&1
  STATUS="$?"
elif command -v grub-mkconfig >/dev/null 2>&1; then
  REGENERATED="grub-mkconfig -o $GRUB_CONFIG"
  grub-mkconfig -o "$GRUB_CONFIG" >> "$XOS_LOG" 2>&1
  STATUS="$?"
else
  xsoft_fail "grub-mkconfig is not here, so the boot menu still has the old command line"
  xlog "   run it by hand: grub-mkconfig -o $GRUB_CONFIG"
  exit 0
fi

if [ "$STATUS" != "0" ]; then
  xsoft_fail "$REGENERATED failed; the boot menu still has the old command line"
  exit 0
fi

# The real proof is the generated file, not the defaults file we edited.
if grep -q -- "${MISSING[0]}" "$XOS_ROOT$GRUB_CONFIG" 2>/dev/null; then
  xlog "   the boot menu now passes them to the kernel"
else
  xsoft_fail "the regenerated boot config does not contain them; check $XOS_ROOT$GRUB_CONFIG"
fi

exit 0
