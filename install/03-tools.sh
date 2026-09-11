#!/usr/bin/env bash
#
# The command line tools XOS itself reaches for.
#
# These are not a matter of taste. The tool layer calls ripgrep to search, jq to
# read JSON, ffmpeg to handle media and tesseract to read an image, so they are
# part of what XOS can do rather than decoration on someone's shell.

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/lib.sh"

xstep "Tools"

pin_install_all ripgrep fd fzf jq yq bat eza zoxide sqlite httpie rclone
pin_install_all ffmpeg imagemagick poppler tesseract tesseract-data-eng pandoc-cli

xlog ""
xlog "   XOS calls these directly, so a missing one narrows what it can do."
exit 0
