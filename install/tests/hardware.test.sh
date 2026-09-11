#!/usr/bin/env bash
#
# Tests for install/00-hardware.sh.
#
# An installer nobody can run twice is how machines end up unbootable, so this
# runs the real script against a fake root, a fake package manager and captured
# inventories. Nothing here touches the machine it runs on.
#
#   ./install/tests/hardware.test.sh

set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
INSTALL_DIR="$HERE/.."
SCRIPT="$INSTALL_DIR/00-hardware.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASSED=0
FAILED=0

pass() { printf '  ok    %s\n' "$1"; PASSED=$((PASSED + 1)); }
fail() { printf '  FAIL  %s\n' "$1"; printf '        %s\n' "${2:-}"; FAILED=$((FAILED + 1)); }

check() { # name, condition-description, actual-test-result
  if [ "$3" = "0" ]; then pass "$1"; else fail "$1" "$2"; fi
}

# A package manager that installs nothing and can be told to refuse by name.
make_package_manager() {
  local path="$WORK/fake-pacman"
  cat > "$path" <<'STUB'
#!/usr/bin/env bash
# Refuses anything named in XOS_TEST_REFUSE, succeeds otherwise.
action="${1:-}"
shift 2>/dev/null
for argument in "$@"; do
  case "$argument" in
    -*) continue ;;
  esac
  for refused in ${XOS_TEST_REFUSE:-}; do
    if [ "$argument" = "$refused" ] || [ "$refused" = "ALL" ]; then
      echo "fake-pacman: refusing $argument" >&2
      exit 1
    fi
  done
done
case "$action" in
  -Qq) exit 1 ;;   # nothing is ever already installed
  *)   echo "fake-pacman $action $*"; exit 0 ;;
esac
STUB
  chmod +x "$path"
  printf '%s' "$path"
}

PACKAGE_MANAGER="$(make_package_manager)"

# Run the script in a throwaway root and print where its log went.
run_install() { # inventory-file, extra env assignments as NAME=VALUE...
  local inventory="$1"; shift
  local root="$WORK/root-$RANDOM"
  mkdir -p "$root/etc" "$root/var/log"
  printf '[options]\nHoldPkg = pacman glibc\n' > "$root/etc/pacman.conf"
  env XOS_INSTALL_ROOT="$root" \
      XOS_PACKAGE_MANAGER="$PACKAGE_MANAGER" \
      XOS_HARDWARE_JSON="$inventory" \
      XOS_DKMS_DIR="$WORK/no-such-media" \
      "$@" \
      bash "$SCRIPT" --assume-no > "$root/stdout.txt" 2>&1
  echo "$?" > "$root/exit"
  printf '%s' "$root"
}

# ---------------------------------------------------------------- the script

printf '\nThe script itself\n'

check "it parses" "the installer has a syntax error, so it would do nothing at all" \
  "$(bash -n "$SCRIPT" 2>/dev/null && echo 0 || echo 1)"
check "it does not use set -e" "one failing command would abort the install" \
  "$(grep -qE '^set -[a-z]*e' "$SCRIPT" && echo 1 || echo 0)"

# ---------------------------------------------------------------- fixtures

cat > "$WORK/unsupported-wifi.json" <<'JSON'
{
  "profile": "cpu",
  "inventory": {
    "cpu": {"model": "Intel Core i5-3320M", "cores": 2, "threads": 4,
            "microarchitecture_level": 2, "baseline_ok": true},
    "memory": {"total_mb": 8000, "available_mb": 6000},
    "firmware": {"mode": "bios", "secure_boot": null},
    "gpus": [{"class": "display", "description": "Intel 3rd Gen Core graphics",
              "vendor_id": "8086", "device_id": "0166", "kernel_driver": "i915"}],
    "network": [{"class": "network", "description": "Realtek RTL8821CE Wireless Network Adapter",
                 "vendor_id": "10ec", "device_id": "c821"}],
    "storage": [], "displays": [], "audio": [], "bluetooth": []
  },
  "resolve": {
    "display": [{"vendor": "Intel", "vendor_id": "8086", "device_id": "0166",
                 "class": "display", "driver": "mesa", "firmware": ["linux-firmware"],
                 "kernel_parameters": [], "fallback": ["i915", "vesa"],
                 "confidence": "known"}],
    "network": [{"vendor": "Realtek", "vendor_id": "10ec", "device_id": "c821",
                 "class": "network", "driver": "in-tree", "firmware": ["linux-firmware"],
                 "kernel_parameters": [], "fallback": ["r8169", "r8168-dkms"],
                 "confidence": "known"}]
  }
}
JSON

cat > "$WORK/gtx1080.json" <<'JSON'
{
  "profile": "local",
  "inventory": {
    "cpu": {"model": "Intel Core i7-4790K", "cores": 4, "threads": 8,
            "microarchitecture_level": 3, "baseline_ok": true},
    "memory": {"total_mb": 16000, "available_mb": 11000},
    "firmware": {"mode": "uefi", "secure_boot": false},
    "gpus": [{"class": "display", "description": "NVIDIA GP104 [GeForce GTX 1080]",
              "vendor_id": "10de", "device_id": "1b80",
              "kernel_driver": "nvidia", "vram_mb": 8192}],
    "network": [{"class": "network", "description": "Realtek RTL8111",
                 "vendor_id": "10ec", "device_id": "8168"}],
    "storage": [], "displays": [], "audio": [], "bluetooth": []
  },
  "resolve": {
    "display": [{"vendor": "NVIDIA", "vendor_id": "10de", "device_id": "1b80",
                 "class": "display", "architecture": "Pascal",
                 "driver": "nvidia-580xx-dkms", "utils": "nvidia-580xx-utils",
                 "branch": "580",
                 "firmware": ["linux-firmware"],
                 "kernel_parameters": ["nvidia_drm.modeset=1"],
                 "fallback": ["nouveau", "vesa"], "confidence": "confirmed"}],
    "network": [{"vendor": "Realtek", "vendor_id": "10ec", "device_id": "8168",
                 "class": "network", "driver": "in-tree",
                 "firmware": ["linux-firmware"], "kernel_parameters": [],
                 "fallback": ["r8169", "r8168-dkms"], "confidence": "known"}]
  },
  "report": {
    "format": "xos-hardware-report",
    "format_version": 1,
    "cpu_model": "Intel Core i7-4790K",
    "cpu_level": 3,
    "memory_mb": 16000,
    "firmware": "uefi",
    "profile": "local",
    "display": [{"vendor_id": "10de", "device_id": "1b80", "class": "display",
                 "kernel_driver": "nvidia", "resolved_driver": "nvidia-580xx-dkms",
                 "branch": "580", "confidence": "confirmed"}],
    "network": [{"vendor_id": "10ec", "device_id": "8168", "class": "network",
                 "resolved_driver": "in-tree", "confidence": "known"}]
  }
}
JSON

cat > "$WORK/unknown-gpu.json" <<'JSON'
{
  "profile": "api-only",
  "inventory": {
    "cpu": {"model": "AMD Athlon II X2", "cores": 2, "threads": 2,
            "microarchitecture_level": 1, "baseline_ok": true},
    "memory": {"total_mb": 4000, "available_mb": 2500},
    "firmware": {"mode": "bios", "secure_boot": null},
    "gpus": [{"class": "display", "description": "Something nobody has seen",
              "vendor_id": "dead", "device_id": "beef", "kernel_driver": "weird_module"}],
    "network": [], "storage": [], "displays": [], "audio": [], "bluetooth": []
  },
  "resolve": {
    "display": [{"vendor_id": "dead", "device_id": "beef", "class": "display",
                 "driver": "unknown", "firmware": [], "kernel_parameters": [],
                 "fallback": ["vesa"], "confidence": "unknown",
                 "kernel_driver_in_use": "weird_module",
                 "why": "not in the database; add a row if you get this working"}],
    "network": []
  }
}
JSON

cat > "$WORK/hybrid.json" <<'JSON'
{
  "profile": "local",
  "inventory": {
    "cpu": {"model": "Intel Core i7-8750H", "cores": 6, "threads": 12,
            "microarchitecture_level": 3, "baseline_ok": true},
    "memory": {"total_mb": 16000, "available_mb": 12000},
    "firmware": {"mode": "uefi", "secure_boot": true},
    "gpus": [
      {"class": "display", "description": "Intel UHD Graphics 630",
       "vendor_id": "8086", "device_id": "3e9b", "kernel_driver": "i915"},
      {"class": "display", "description": "NVIDIA GP107M [GeForce GTX 1050 Ti Mobile]",
       "vendor_id": "10de", "device_id": "1c8c", "kernel_driver": "nvidia", "vram_mb": 4096}
    ],
    "network": [], "storage": [], "displays": [], "audio": [], "bluetooth": []
  },
  "resolve": {
    "display": [
      {"vendor": "Intel", "vendor_id": "8086", "device_id": "3e9b", "class": "display",
       "driver": "mesa", "firmware": ["linux-firmware"], "kernel_parameters": [],
       "fallback": ["i915", "vesa"], "confidence": "known"},
      {"vendor": "NVIDIA", "vendor_id": "10de", "device_id": "1c8c", "class": "display",
       "architecture": "Pascal", "driver": "nvidia-580xx-dkms",
       "utils": "nvidia-580xx-utils", "branch": "580",
       "firmware": ["linux-firmware"], "kernel_parameters": ["nvidia_drm.modeset=1"],
       "fallback": ["nouveau", "vesa"], "confidence": "known"}
    ],
    "network": []
  }
}
JSON

# ---------------------------------------------------------------- the tests

printf '\nThe check: an unsupported wifi chip\n'

ROOT="$(run_install "$WORK/unsupported-wifi.json" XOS_FAKE_WIFI=0 XOS_FAKE_SINK=1)"
LOG="$ROOT/var/log/xos-hardware.log"
EXIT="$(cat "$ROOT/exit")"

check "the install completes" "exited $EXIT" "$([ "$EXIT" = "0" ] && echo 0 || echo 1)"
check "the log says wifi is unavailable, unmistakably" "no WIFI IS NOT AVAILABLE line" \
  "$(grep -q "WIFI IS NOT AVAILABLE" "$LOG" && echo 0 || echo 1)"
check "the log suggests what to do about it" "no suggestion in the log" \
  "$(grep -qi "USB tethering" "$LOG" && echo 0 || echo 1)"
check "the log says the install is continuing" "the person is left guessing" \
  "$(grep -qi "install is continuing" "$LOG" && echo 0 || echo 1)"
check "a graphics driver was still settled" "no display line in the report" \
  "$(grep -q "display:.*->" "$LOG" && echo 0 || echo 1)"
check "wifi appears in the report section" "not in the report" \
  "$(grep -q "wifi: unavailable" "$LOG" && echo 0 || echo 1)"

printf '\nNothing may abort the install\n'

ROOT="$(run_install "$WORK/unsupported-wifi.json" XOS_TEST_REFUSE=ALL XOS_FAKE_WIFI=0 XOS_FAKE_SINK=0)"
LOG="$ROOT/var/log/xos-hardware.log"
check "every single package failing still completes" "exited $(cat "$ROOT/exit")" \
  "$([ "$(cat "$ROOT/exit")" = "0" ] && echo 0 || echo 1)"
check "graphics still ends somewhere that shows a picture" "no terminal fallback" \
  "$(grep -qE "display:.*-> (vesa|modesetting)" "$LOG" && echo 0 || echo 1)"
check "a missing audio sink is logged, not fatal" "audio not reported" \
  "$(grep -q "audio:" "$LOG" && echo 0 || echo 1)"

ROOT="$(run_install "$WORK/no-such-file.json")"
check "no inventory at all still completes" "exited $(cat "$ROOT/exit")" \
  "$([ "$(cat "$ROOT/exit")" = "0" ] && echo 0 || echo 1)"

printf '\nThe reference machine\n'

ROOT="$(run_install "$WORK/gtx1080.json" XOS_FAKE_WIFI=1 XOS_FAKE_SINK=1)"
LOG="$ROOT/var/log/xos-hardware.log"
check "the matched branch is installed" "nvidia-580xx-dkms not installed" \
  "$(grep -q "installing nvidia-580xx-dkms" "$LOG" && echo 0 || echo 1)"
check "it is pinned so an update cannot break it" "not pinned" \
  "$(grep -q "nvidia-580xx-dkms" "$ROOT/etc/pacman.conf" && echo 0 || echo 1)"
check "the pin is an IgnorePkg line" "pinned the wrong way" \
  "$(grep -qE "^IgnorePkg = nvidia-580xx-dkms" "$ROOT/etc/pacman.conf" && echo 0 || echo 1)"
check "the kernel parameter is carried through" "modeset parameter missing" \
  "$(grep -q "nvidia_drm.modeset=1" "$ROOT/etc/xos/kernel-parameters" && echo 0 || echo 1)"
check "it is recorded as a first choice, not a fallback" "recorded as a fallback" \
  "$(grep -q "display:.*(first choice)" "$LOG" && echo 0 || echo 1)"
check "firmware is installed unconditionally" "linux-firmware missing" \
  "$(grep -q "firmware: linux-firmware installed" "$LOG" && echo 0 || echo 1)"
check "sof-firmware too" "sof-firmware missing" \
  "$(grep -q "firmware: sof-firmware installed" "$LOG" && echo 0 || echo 1)"

check "the userspace is installed alongside the kernel module" "nvidia-580xx-utils never installed" \
  "$(grep -q "installing nvidia-580xx-utils" "$LOG" && echo 0 || echo 1)"
check "the userspace is pinned with the driver" "a mismatched pair does not load" \
  "$(grep -q "nvidia-580xx-utils" "$ROOT/etc/pacman.conf" && echo 0 || echo 1)"

printf '\nThe report says what happened, not what was attempted\n'

check "bluetooth is not claimed as enabled when it was not" "the report claims something that did not happen" \
  "$(grep -q "could not enable bluetooth.service" "$LOG" && grep -q "bluetooth: bluez installed and the service enabled" "$LOG" && echo 1 || echo 0)"

printf '\nA device nobody has seen\n'

ROOT="$(run_install "$WORK/unknown-gpu.json" XOS_FAKE_WIFI=1 XOS_FAKE_SINK=1)"
LOG="$ROOT/var/log/xos-hardware.log"
check "no driver is guessed at" "something was guessed" \
  "$(grep -q "no driver is guessed at" "$LOG" && echo 0 || echo 1)"
check "it falls back rather than failing" "no fallback recorded" \
  "$(grep -qE "display:.*-> vesa" "$LOG" && echo 0 || echo 1)"
check "the module already driving it is not installed as a package" "weird_module was installed" \
  "$(grep -q "installing weird_module" "$LOG" && echo 1 || echo 0)"

printf '\nIn-tree drivers are not looked for in the package manager\n'

# There is no package called virtio-gpu. Asking for one fails, and the install
# log then reports a failure for something that is not a failure and was never
# going to be one.
INTREE_CASE="$(grep -oE '^ +nouveau\|[a-z0-9|_-]+\)' "$INSTALL_DIR/00-hardware.sh" | head -1)"
for driver in virtio-gpu bochs-drm vmwgfx vboxvideo hyperv_drm mgag200 ast; do
  check "$driver is treated as in-tree" "the installer would try to install a kernel module" \
    "$(printf '%s' "$INTREE_CASE" | grep -q -- "$driver" && echo 0 || echo 1)"
done

printf '\nHybrid graphics\n'

ROOT="$(run_install "$WORK/hybrid.json" XOS_FAKE_WIFI=1 XOS_FAKE_SINK=1)"
LOG="$ROOT/var/log/xos-hardware.log"
check "the display is driven from the integrated GPU" "no hybrid decision logged" \
  "$(grep -q "the display comes from the integrated GPU" "$LOG" && echo 0 || echo 1)"
check "the discrete card is left headless for compute" "not left headless" \
  "$(grep -q "leaving it headless for compute" "$LOG" && echo 0 || echo 1)"

printf '\nThe compatibility database\n'

ROOT="$(run_install "$WORK/gtx1080.json" XOS_FAKE_WIFI=1 XOS_FAKE_SINK=1)"
LOG="$ROOT/var/log/xos-hardware.log"
REPORT="$ROOT/var/log/xos-hardware-report.json"
check "it shows exactly what would be sent" "the contents were not shown" \
  "$(grep -q "exactly what would be sent" "$LOG" && echo 0 || echo 1)"
check "nothing is sent without a yes" "it sent something" \
  "$(grep -qi "Not sending anything" "$LOG" && echo 0 || echo 1)"
check "the report is valid json" "malformed report" \
  "$(python3 -c "import json,sys; json.load(open(sys.argv[1]))" "$REPORT" >/dev/null 2>&1 && echo 0 || echo 1)"
check "the report carries the device ids the database is keyed on" "no ids in the report" \
  "$(grep -q '"device_id": "1b80"' "$REPORT" && echo 0 || echo 1)"
check "the report carries no hostname" "a hostname leaked into the report" \
  "$(grep -qi "hostname" "$REPORT" && echo 1 || echo 0)"
check "the report carries no serial number" "a serial number leaked" \
  "$(grep -qi "serial" "$REPORT" && echo 1 || echo 0)"

printf '\nA dry run changes nothing\n'

ROOT_DIR="$WORK/dry"; mkdir -p "$ROOT_DIR/etc" "$ROOT_DIR/var/log"
printf '[options]\nHoldPkg = pacman glibc\n' > "$ROOT_DIR/etc/pacman.conf"
BEFORE="$(md5sum "$ROOT_DIR/etc/pacman.conf" | cut -d' ' -f1)"
env XOS_INSTALL_ROOT="$ROOT_DIR" XOS_PACKAGE_MANAGER="$PACKAGE_MANAGER" \
    XOS_HARDWARE_JSON="$WORK/gtx1080.json" XOS_FAKE_WIFI=1 XOS_FAKE_SINK=1 \
    bash "$SCRIPT" --dry-run > "$ROOT_DIR/stdout.txt" 2>&1
DRY_EXIT="$?"
AFTER="$(md5sum "$ROOT_DIR/etc/pacman.conf" | cut -d' ' -f1)"
check "a dry run completes" "exited $DRY_EXIT" "$([ "$DRY_EXIT" = "0" ] && echo 0 || echo 1)"
check "a dry run does not touch pacman.conf" "pacman.conf was modified" \
  "$([ "$BEFORE" = "$AFTER" ] && echo 0 || echo 1)"
check "a dry run says what it would have done" "nothing reported" \
  "$(grep -q "would install nvidia-580xx-dkms" "$ROOT_DIR/var/log/xos-hardware.log" && echo 0 || echo 1)"

printf '\n%s passed, %s failed\n\n' "$PASSED" "$FAILED"
[ "$FAILED" = "0" ]
