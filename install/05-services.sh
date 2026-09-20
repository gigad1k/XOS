#!/usr/bin/env bash
#
# Open WebUI, SearXNG and the OpenClaw gateway.
#
# # Why none of these start at boot
#
# XOS targets machines with 8GB of RAM. Open WebUI is Python, SearXNG is Python,
# the OpenClaw gateway is Node. Three language runtimes resident from boot is
# somewhere around a gigabyte of memory spent permanently on things used for a
# few minutes a week, on a machine that does not have a gigabyte to spare.
#
# So each one is socket-activated. A socket unit holds the port from boot, which
# costs nothing. The first connection starts the service; after a stretch with
# no connections it stops again and gives the memory back. From the outside
# nothing changed: the port answers. The first request is slower.
#
# The pattern is a socket unit, a proxy that systemd starts on demand
# (systemd-socket-proxyd, whose --exit-idle-time does the stopping), and the
# real daemon behind it on a private port with StopWhenUnneeded so it goes away
# with the proxy. It is more units than starting things at boot, and it is the
# difference between XOS being usable on the machines it is for and not.
#
# Two things do start at boot: xosd, and llama-server. Both are the reason the
# machine is running at all.
#
# One exception, which is not a compromise but a fact about inbound messages: if
# messaging is configured, the OpenClaw gateway starts at boot. A socket cannot
# activate a listener for a message that arrives at a listener that is not
# listening.

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$HERE/lib.sh"

# How long a service sits idle before giving its memory back. Long enough that
# someone reading a conversation does not get a restart mid-thought; short
# enough to matter on an 8GB machine.
IDLE="15min"

xstep "Service prerequisites"
pin_install_all python-pipx socat || xsoft_fail "some service prerequisites are missing"

# One socket-activated service, in the three units it takes.
socket_activated() { # name, public port, private port, description
  local name="$1" public="$2" private="$3" description="$4"

  install_user_unit "$name.socket" <<UNIT
# Written by XOS.
#
# This holds port $public from boot, which costs nothing. The service behind it
# does not exist until someone connects.
[Unit]
Description=$description socket

[Socket]
# Loopback only, and that is the whole of it: these are windows onto this
# machine and have no business being reachable from anywhere else. The address
# does the work. No BindIPv6Only here, because it applies to IPv6 sockets and
# this is an IPv4 literal, so it would only imply a protection it is not giving.
ListenStream=127.0.0.1:$public
Accept=no

[Install]
WantedBy=sockets.target
UNIT

  install_user_unit "$name-proxy.service" <<UNIT
# Written by XOS.
#
# Started by the socket, on the first connection. It forwards to the real
# service and, after $IDLE with nobody connected, exits. The real service is
# StopWhenUnneeded, so it goes with it and the memory comes back.
[Unit]
Description=$description on demand
Requires=$name.socket
After=$name.socket
Requires=$name.service
After=$name.service

[Service]
ExecStart=/usr/lib/systemd/systemd-socket-proxyd --exit-idle-time=$IDLE 127.0.0.1:$private
PrivateTmp=yes
UNIT
}

# ---------------------------------------------------------------- Open WebUI

xstep "Open WebUI"

socket_activated "xos-openwebui" 8081 18081 "Open WebUI"

install_user_unit "xos-openwebui.service" <<'UNIT'
# Written by XOS.
#
# Deliberately not enabled. It is started by its proxy and stopped when nothing
# needs it.
[Unit]
Description=Open WebUI
StopWhenUnneeded=yes

[Service]
Type=simple
Environment=HOST=127.0.0.1
Environment=PORT=18081
Environment=WEBUI_AUTH=False
ExecStart=%h/.local/bin/open-webui serve
Restart=no
UNIT

if [ "$XOS_DRY_RUN" = "1" ]; then
  xlog "   would install open-webui"
elif in_target command -v pipx >/dev/null 2>&1; then
  in_target pipx install open-webui >> "$XOS_LOG" 2>&1 \
    || xsoft_fail "Open WebUI did not install; the chat interface will be the CLI"
else
  xsoft_fail "pipx is missing, so Open WebUI was skipped"
fi

# The XOS theme is layered as an extra stylesheet and never patched into Open
# WebUI's own files, so an upstream update keeps applying. It also keeps Open
# WebUI's own branding visible, which its licence requires.
if [ -f "$HERE/../desktop/openwebui/xos.css" ]; then
  layer_into "etc/xos/openwebui/xos.css" < "$HERE/../desktop/openwebui/xos.css"
fi

# ---------------------------------------------------------------- SearXNG

xstep "SearXNG"

socket_activated "xos-searxng" 8888 18888 "SearXNG"

install_user_unit "xos-searxng.service" <<'UNIT'
# Written by XOS.
[Unit]
Description=SearXNG
StopWhenUnneeded=yes

[Service]
Type=simple
Environment=SEARXNG_BIND_ADDRESS=127.0.0.1
Environment=SEARXNG_PORT=18888
ExecStart=%h/.local/bin/searxng-run
Restart=no
UNIT

if [ "$XOS_DRY_RUN" = "1" ]; then
  xlog "   would install searxng"
elif in_target command -v pipx >/dev/null 2>&1; then
  in_target pipx install searxng >> "$XOS_LOG" 2>&1 \
    || xsoft_fail "SearXNG did not install; search will need an API"
fi

# ---------------------------------------------------------------- OpenClaw

xstep "OpenClaw gateway"

# Node, not Bun. The OpenClaw documentation flags Bun as unstable for WhatsApp
# and Telegram sessions, and a messaging gateway that drops sessions is worse
# than no messaging gateway: someone sends a message and believes it arrived.
if [ "$XOS_DRY_RUN" = "1" ]; then
  xlog "   would install the OpenClaw gateway under Node"
elif in_target command -v npm >/dev/null 2>&1; then
  in_target npm install -g openclaw >> "$XOS_LOG" 2>&1 \
    || xsoft_fail "the OpenClaw gateway did not install; messaging will be unavailable"
else
  xsoft_fail "npm is missing, so the OpenClaw gateway was skipped"
fi

socket_activated "xos-openclaw" 8090 18090 "OpenClaw gateway"

install_user_unit "xos-openclaw.service" <<'UNIT'
# Written by XOS.
#
# Runs under Node. The OpenClaw documentation flags Bun as unstable for WhatsApp
# and Telegram sessions, and a dropped session means someone believes a message
# was delivered when it was not.
[Unit]
Description=OpenClaw gateway
StopWhenUnneeded=yes

[Service]
Type=simple
Environment=PORT=18090
Environment=HOST=127.0.0.1
ExecStart=/usr/bin/node /usr/lib/node_modules/openclaw/bin/openclaw.js serve
Restart=on-failure
RestartSec=5
UNIT

# The exception, and the reason for it.
xstep "What starts at boot"

MESSAGING_CONFIGURED=0
if [ -f "$XOS_ROOT/etc/xos/messaging.toml" ] || [ -n "${XOS_MESSAGING:-}" ]; then
  MESSAGING_CONFIGURED=1
fi

if [ "$MESSAGING_CONFIGURED" = "1" ]; then
  # An inbound message arrives at a listener or it does not arrive. Socket
  # activation cannot help: there is nobody to make the first connection.
  xlog "   messaging is configured, so the OpenClaw gateway starts at boot"
  xlog "   an inbound message cannot socket-activate a listener that is not listening"
  enable_user_unit "xos-openclaw.service"
else
  xlog "   messaging is not configured, so the gateway waits to be asked"
  enable_user_unit "xos-openclaw.socket"
fi

enable_user_unit "xos-openwebui.socket"
enable_user_unit "xos-searxng.socket"

xlog ""
xlog "   Enabled at boot: the three sockets, which hold ports and nothing else."
xlog "   Started at boot: xosd and llama-server, and nothing else."
xlog "   Everything else starts on the first connection and stops after $IDLE idle."
exit 0
