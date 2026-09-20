#!/usr/bin/env bash
#
# The XOS desktop layer.
#
# Every file here goes beside an Omarchy file, never over one. Omarchy owns
# hyprland.conf; XOS writes xos.conf next to it and adds a single `source =`
# line. Omarchy owns its waybar config; XOS adds its module through a drop-in.
#
# That is not fastidiousness. XOS layers over Omarchy precisely so that Omarchy
# updates keep applying, and the moment XOS edits an Omarchy file in place, the
# next update either reverts XOS or conflicts with it, and the reason for
# layering at all is gone. One `source =` line is also the smallest possible
# footprint in someone else's config, and anyone who wants XOS gone can delete
# it by hand.

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$HERE/lib.sh"

DESKTOP="$HERE/../desktop"
CONFIG="etc/skel/.config"

xstep "Desktop packages"
pin_install_all hyprland waybar foot plymouth || xsoft_fail "some desktop packages are missing"

xstep "Hyprland"

if [ -f "$DESKTOP/hypr/xos.conf" ]; then
  layer_into "$CONFIG/hypr/xos.conf" < "$DESKTOP/hypr/xos.conf"
  # The one line that pulls it all in. Idempotent, and removable by hand.
  include_once "$CONFIG/hypr/hyprland.conf" "source = ~/.config/hypr/xos.conf"
else
  xsoft_fail "desktop/hypr/xos.conf is missing"
fi

xstep "The status bar"

# Waybar has no include directive, so the XOS module lives in its own config
# and style, and Omarchy's are left untouched. Whichever the session launches,
# neither has been edited.
if [ -f "$DESKTOP/waybar/config.jsonc" ]; then
  layer_into "$CONFIG/waybar/xos/config.jsonc" < "$DESKTOP/waybar/config.jsonc"
fi
if [ -f "$DESKTOP/waybar/style.css" ]; then
  layer_into "$CONFIG/waybar/xos/style.css" < "$DESKTOP/waybar/style.css"
fi

layer_into "$CONFIG/waybar/xos/README" <<'NOTE'
Written by XOS.

Omarchy's waybar config is untouched. To use the XOS bar instead, launch waybar
with:

  waybar -c ~/.config/waybar/xos/config.jsonc -s ~/.config/waybar/xos/style.css

The Hyprland layer does this already. Delete this directory and the XOS
`source =` line in hyprland.conf, and nothing of XOS remains in the desktop.
NOTE

xstep "The terminal"
if [ -f "$DESKTOP/terminal/xos-foot.ini" ]; then
  layer_into "$CONFIG/foot/foot.ini" < "$DESKTOP/terminal/xos-foot.ini"
fi

xstep "Boot splash"

if [ -d "$DESKTOP/plymouth/xos" ]; then
  if [ "$XOS_DRY_RUN" = "1" ]; then
    xlog "   would install the XOS plymouth theme"
  else
    mkdir -p "$XOS_ROOT/usr/share/plymouth/themes/xos" 2>/dev/null
    cp -r "$DESKTOP/plymouth/xos/." "$XOS_ROOT/usr/share/plymouth/themes/xos/" 2>/dev/null \
      && xlog "   installed the plymouth theme"
    # Setting it is a separate decision from installing it: on a machine with a
    # slow disk, a splash that hides a long boot is worse than watching it.
    if in_target command -v plymouth-set-default-theme >/dev/null 2>&1; then
      in_target plymouth-set-default-theme xos >> "$XOS_LOG" 2>&1 \
        || xsoft_fail "could not set the plymouth theme"
    fi
  fi
fi

xlog ""
xlog "   Nothing Omarchy wrote has been modified. One source line was added to"
xlog "   hyprland.conf, and removing it removes XOS from the desktop."
exit 0
