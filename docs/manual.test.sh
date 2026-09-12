#!/usr/bin/env bash
#
# Every command docs/manual.md tells somebody to type, checked against the
# binary that exists.
#
# Documentation drifts silently. A manual that names a flag which was renamed
# six months ago is worse than no manual: somebody types it, it fails, and they
# conclude the software is broken rather than the sentence.
#
#   cargo build && ./docs/manual.test.sh

export PATH="$HOME/.cargo/bin:$PATH"
HERE="$(cd "$(dirname "$0")" && pwd)"
cd "$HERE/.." || exit 1
XOS=./target/debug/xos

if [ ! -x "$XOS" ]; then
  echo "  build it first: cargo build"
  exit 1
fi
FAIL=0

ok()   { printf '  ok    %s\n' "$1"; }
bad()  { printf '  FAIL  %s\n' "$1"; printf '        %s\n' "$2"; FAIL=1; }

# Parse-only: --help on the exact shape, so nothing is actually run.
shape() { # description, args...
  local what="$1"; shift
  if "$XOS" "$@" --help >/dev/null 2>&1; then
    ok "$what"
  else
    bad "$what" "$($XOS "$@" --help 2>&1 | head -2 | tr '\n' ' ')"
  fi
}

echo "Commands the manual names"
for c in halt resume status chat setup escalations mode memory bar pulse goal \
         undo journal supervisor policy export import hardware spend vault; do
  shape "xos $c" "$c"
done

echo
echo "Subcommands the manual names"
shape "xos memory search"   memory search
shape "xos memory stats"    memory stats
shape "xos memory write"    memory write
shape "xos memory promote"  memory promote
shape "xos goal new"        goal new
shape "xos goal list"       goal list
shape "xos goal show"       goal show
shape "xos goal advance"    goal advance
shape "xos policy log"      policy log
shape "xos policy test"     policy test
shape "xos pulse status"    pulse status
shape "xos pulse tasks"     pulse tasks
shape "xos vault add"       vault add
shape "xos vault list"      vault list
shape "xos vault remove"    vault remove

echo
echo "Flags the manual names"
flag() { # description, command..., flag
  local what="$1"; shift
  local wanted="${!#}"
  local args=("${@:1:$#-1}")
  if "$XOS" "${args[@]}" --help 2>&1 | grep -q -- "$wanted"; then
    ok "$what"
  else
    bad "$what" "$wanted is not offered"
  fi
}
flag "xos hardware --json"      hardware --json
flag "xos hardware --device"    hardware --device
flag "xos hardware --submit"    hardware --submit
flag "xos setup --skip-all"     setup --skip-all
flag "xos journal --limit"      journal --limit
flag "xos undo --last"          undo --last
flag "xos spend --days"         spend --days
flag "xos escalations --limit"  escalations --limit
flag "xos memory write --tier"  memory write --tier
flag "xos memory promote --from" memory promote --from
flag "xos memory promote --to"  memory promote --to
flag "xos memory search --tier" memory search --tier

echo
echo "Things the manual promises about the install scripts"
grep -q -- '--only' install/install.sh && ok "install.sh --only" || bad "install.sh --only" "not there"
grep -q -- '--encrypt' install/base.sh && ok "base.sh --encrypt" || bad "base.sh --encrypt" "not there"
grep -q -- '--dry-run' install/base.sh && ok "base.sh --dry-run" || bad "base.sh --dry-run" "not there"
grep -q 'gigad1k/XOS' install/boot.sh && ok "the repository URL" || bad "the repository URL" "wrong"
grep -q '7777' xosd/src/config.rs && ok "Mission Control on 7777" || bad "Mission Control port" "not 7777"
for word in strict standard permissive; do
  grep -q "\"$word\"" xosd/src/policy/mod.rs && ok "strictness: $word" || bad "strictness: $word" "not a real value"
done
for word in aggressive-local balanced best-quality; do
  grep -rq -- "$word" xosd/src/router/ && ok "cost mode: $word" || bad "cost mode: $word" "not a real value"
done

echo
echo "Commands the headless manual names"
for pair in "goal:advance" "memory:search" "policy:log" "pulse:status" "vault:add"; do
  shape "xos ${pair%%:*} ${pair##*:}" "${pair%%:*}" "${pair##*:}"
done
for f in halt resume status chat setup export import journal undo hardware; do
  shape "xos $f" "$f"
done
grep -q 'loginctl enable-linger' "$HERE/headless.md" && ok "lingering is documented" || bad "lingering" "not mentioned"
grep -q 'journalctl --user -u xosd' "$HERE/headless.md" && ok "the daemon log is named" || bad "journalctl" "not mentioned"
grep -q 'install.sh --only' "$HERE/headless.md" && ok "--only is documented" || bad "--only" "not mentioned"
grep -q -- '--only' "$HERE/../install/install.sh" && ok "--only exists" || bad "--only" "install.sh does not offer it"

echo
[ "$FAIL" = "0" ] && echo "Every command in the manual exists." || echo "THE MANUAL IS WRONG SOMEWHERE."
exit "$FAIL"
