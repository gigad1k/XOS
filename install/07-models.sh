#!/usr/bin/env bash
#
# Pull the local model this machine can actually run.
#
# The choice comes from verified-models.json, filtered by what `xos hardware`
# says the machine is. That filtering is the whole job: a model that does not
# fit produces a machine that swaps, stutters and feels broken, and the person
# concludes that local models do not work rather than that this one did not fit.
#
# An api-only machine gets nothing, on purpose. Downloading four gigabytes onto
# a machine that cannot run it is a waste of somebody's bandwidth and disk.

set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
. "$HERE/lib.sh"

MODELS="$XOS_ROOT/opt/xos/models"
CATALOGUE="${XOS_MODELS_DB:-$HERE/../verified-models.json}"

xstep "The model catalogue"

if [ ! -f "$CATALOGUE" ]; then
  xsoft_fail "verified-models.json is missing, so no model can be chosen"
  exit 0
fi

xstep "What this machine can run"

REPORT="${XOS_HARDWARE_JSON:-}"
if [ -z "$REPORT" ] && command -v xos >/dev/null 2>&1; then
  REPORT="$(mktemp)"
  xos hardware --json > "$REPORT" 2>/dev/null || REPORT=""
fi

CHOICE="$(python3 - "$CATALOGUE" "${REPORT:-}" <<'PY'
import json, sys

catalogue_path, report_path = sys.argv[1], (sys.argv[2] if len(sys.argv) > 2 else "")

try:
    catalogue = json.load(open(catalogue_path))
except Exception:
    sys.exit(1)

# Without a report there is nothing to filter on, and filtering on zeros would
# report that nothing fits — which is a different statement from not knowing,
# and one that might well be false.
profile, vram, memory = None, 0, 0
if report_path:
    try:
        report = json.load(open(report_path))
        profile = report.get("profile") or "cpu"
        inventory = report.get("inventory", {})
        vram = max(
            [g.get("vram_mb") or 0 for g in inventory.get("gpus", [])] or [0]
        )
        memory = inventory.get("memory", {}).get("total_mb", 0)
    except Exception:
        profile = None

if profile is None:
    print("UNKNOWN no hardware report")
    sys.exit(0)

# A machine that cannot run one gets none. Pulling gigabytes onto it wastes
# somebody's bandwidth and tells them nothing true about local models.
if profile == "api-only":
    print("NONE api-only")
    sys.exit(0)

candidates = [
    entry
    for entry in catalogue.get("entries", [])
    if profile in entry.get("profiles", [])
    and vram >= (entry.get("minimum_vram_mb", 0) if profile == "local" else 0)
    and memory >= entry.get("minimum_memory_mb", 0)
]
if not candidates:
    print("NONE nothing fits")
    sys.exit(0)

# Rank 1 is the top-ranked model, and a confirmed row beats an unproven one of
# the same rank: somebody has actually run it.
best = sorted(
    candidates,
    key=lambda e: (e.get("rank", 99), 0 if e.get("confidence") == "confirmed" else 1),
)[0]
print(
    "%s\t%s\t%s\t%s\t%s"
    % (best["id"], best["repository"], best["file"], best.get("name", best["id"]), best.get("confidence", ""))
)
PY
)" || CHOICE=""

case "${CHOICE%% *}" in
  UNKNOWN)
    # Not the same as "nothing fits". Saying so would tell somebody their
    # machine cannot run a local model when it very likely can.
    xwarn "There is no hardware report, so no model can be chosen yet."
    xlog "   Nothing has been downloaded and nothing has been ruled out."
    xlog "   Start xosd, then run this step again:"
    xlog "     bash install/install.sh --only 07-models.sh"
    exit 0
    ;;
  NONE)
    xlog "   ${CHOICE#NONE }"
    xlog "   XOS will work through an API. Nothing has been downloaded."
    exit 0
    ;;
  "")
    xwarn "the catalogue could not be read, so no model was chosen"
    exit 0
    ;;
esac

MODEL_ID="$(printf '%s' "$CHOICE" | cut -f1)"
REPOSITORY="$(printf '%s' "$CHOICE" | cut -f2)"
FILE="$(printf '%s' "$CHOICE" | cut -f3)"
MODEL_NAME="$(printf '%s' "$CHOICE" | cut -f4)"
CONFIDENCE="$(printf '%s' "$CHOICE" | cut -f5)"

xlog "   $MODEL_NAME  ($CONFIDENCE)"
xlog "   $REPOSITORY / $FILE"

# Downloading belongs to the machine being built.
#
# uv and the huggingface CLI are installed into the target by 02-runtimes, not
# onto whatever is running this script. Installing from media, neither is on
# PATH here, so both steps below found nothing and skipped - leaving a machine
# whose entire point is a local model without one, on a project whose first
# non-negotiable is that it works offline.
#
# The paths change with it: inside the target, $XOS_ROOT/opt/xos/models is
# /opt/xos/models.
MODELS_THERE="/opt/xos/models"
[ -z "$XOS_ROOT" ] && MODELS_THERE="$MODELS"

in_target() {
  if [ -n "$XOS_ROOT" ] && command -v arch-chroot >/dev/null 2>&1; then
    arch-chroot "$XOS_ROOT" "$@"
  else
    "$@"
  fi
}

xstep "The downloader"
if [ "$XOS_DRY_RUN" = "1" ]; then
  xlog "   would install the huggingface CLI"
elif in_target command -v uv >/dev/null 2>&1; then
  in_target uv tool install "huggingface-hub[cli]" >> "$XOS_LOG" 2>&1 \
    || xsoft_fail "the huggingface CLI did not install"
fi

xstep "Pulling $MODEL_NAME"
if [ "$XOS_DRY_RUN" = "1" ]; then
  xlog "   would download $FILE into $MODELS"
  exit 0
fi

mkdir -p "$MODELS" 2>/dev/null
if in_target command -v hf >/dev/null 2>&1; then
  in_target hf download "$REPOSITORY" "$FILE" --local-dir "$MODELS_THERE" >> "$XOS_LOG" 2>&1 \
    || xsoft_fail "the download did not finish; XOS will use an API until it does"
else
  xsoft_fail "no huggingface CLI, so nothing was downloaded"
fi

# One stable path, so the llama-server unit does not have to know which model
# ended up here.
if [ -f "$MODELS/$FILE" ]; then
  ln -sf "$MODELS/$FILE" "$MODELS/current.gguf" 2>/dev/null
  xlog "   $MODELS/current.gguf now points at $FILE"
  printf '%s\n' "$MODEL_ID" > "$XOS_ROOT/etc/xos/model" 2>/dev/null
fi

exit 0
