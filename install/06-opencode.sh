#!/usr/bin/env bash
#
# OpenCode.
#
# The coding agent XOS hands work to when a goal turns out to be a programming
# job. Installed, never resident: it runs when something calls it and exits.

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$HERE/lib.sh"

xstep "OpenCode"

if [ "$XOS_DRY_RUN" = "1" ]; then
  xlog "   would install opencode"
elif in_target command -v npm >/dev/null 2>&1; then
  in_target npm install -g opencode-ai >> "$XOS_LOG" 2>&1 \
    || xsoft_fail "OpenCode did not install; XOS will do coding work itself, less well"
else
  xsoft_fail "npm is missing, so OpenCode was skipped"
fi

# Its configuration is layered rather than written into anything OpenCode owns,
# so an update to it keeps applying.
layer_into "etc/xos/opencode.json" <<'CONFIG'
{
  "_comment": "Written by XOS. Points OpenCode at the local model first, so a coding question does not become an API bill by default.",
  "provider": {
    "local": {
      "npm": "@ai-sdk/openai-compatible",
      "options": { "baseURL": "http://127.0.0.1:8080/v1" }
    }
  },
  "model": "local/current"
}
CONFIG

xlog ""
xlog "   OpenCode runs when called and exits. Nothing of it is resident."
exit 0
