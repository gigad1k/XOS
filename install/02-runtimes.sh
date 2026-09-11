#!/usr/bin/env bash
#
# Language runtimes and the tools that come with them.
#
# Installed, not started. Nothing here is resident: XOS targets 8GB machines and
# a language runtime sitting idle is a tax paid every second of every day for
# something used a few minutes a week. What runs at boot is decided in
# 05-services.sh, deliberately and in one place.

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/lib.sh"

xstep "Runtimes"

# Node LTS rather than current: OpenClaw and Open WebUI both target LTS, and a
# machine that exists to be reliable is not the place to be early.
pin_install_all nodejs-lts-iron npm pnpm

# Python with uv, which is fast enough to matter on an old CPU. pip is here too
# because some things still assume it.
pin_install_all python python-pip uv

pin_install_all rust go

# Podman rather than Docker: no daemon, so nothing is resident when no container
# is running.
pin_install podman

pin_install_all base-devel git github-cli lazygit

xlog ""
xlog "   None of these run at boot. What does is decided in 05-services.sh."
exit 0
