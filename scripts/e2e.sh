#!/bin/bash
# End-to-end test for the rucksack CLI, driven the way a person drives it.
#
# This takes REAL power leases: it switches system sleep off and back on. It always restores normal
# sleep on exit, including on failure or interrupt, and it never touches your own rucksack state —
# every case runs against a throwaway HOME.
#
# Usage: scripts/e2e.sh [path-to-rucksack]     (default: target/debug/rucksack)
#
# The lease-lifecycle cases pack with `--here`, which is the real flag a tethered user passes when
# rucksack cannot identify their network by itself. They need only a working internet connection.

set -uo pipefail

RUCKSACK=${1:-target/debug/rucksack}
if [ ! -x "$RUCKSACK" ]; then
    echo "no rucksack binary at $RUCKSACK — run: cargo build" >&2
    exit 1
fi
RUCKSACK=$(cd "$(dirname "$RUCKSACK")" && pwd)/$(basename "$RUCKSACK")

WORK=$(mktemp -d "${TMPDIR:-/tmp}/rucksack-e2e.XXXXXX")
PASSED=0
FAILED=0
SKIPPED=0

sleep_disabled() { /usr/bin/pmset -g | /usr/bin/awk '/SleepDisabled/ { print $2 }'; }
gateway() { /sbin/route -n get default 2>/dev/null | /usr/bin/awk '/gateway/ { print $2 }'; }

# A lease left behind would keep this Mac awake indefinitely.
cleanup() {
    for home in "$WORK"/home-*; do
        [ -d "$home" ] || continue
        HOME="$home" "$RUCKSACK" unpack >/dev/null 2>&1
    done
    if [ "$(sleep_disabled)" != "0" ]; then
        echo "!! sleep still disabled; forcing a release" >&2
        HOME="$WORK" "$RUCKSACK" unpack >/dev/null 2>&1
    fi
    /bin/rm -rf "$WORK"
}
trap cleanup EXIT HUP INT TERM

new_home() {
    HOME_DIR="$WORK/home-$(printf '%03d' $((PASSED + FAILED + SKIPPED + 1)))"
    mkdir -p "$HOME_DIR"
    export HOME="$HOME_DIR"
}

ok() { PASSED=$((PASSED + 1)); printf '  \033[32mok\033[0m    %s\n' "$1"; }
skip() { SKIPPED=$((SKIPPED + 1)); printf '  \033[33mskip\033[0m  %s\n' "$1"; }
fail() {
    FAILED=$((FAILED + 1))
    printf '  \033[31mFAIL\033[0m  %s\n' "$1"
    [ -n "${2:-}" ] && printf '%s\n' "$2" | /usr/bin/sed 's/^/          /'
}
check() { if [ "$1" = "0" ]; then ok "$2"; else fail "$2" "${3:-}"; fi; }
contains() { case "$1" in *"$2"*) return 0 ;; *) return 1 ;; esac; }
lines() { printf '%s\n' "$1" | /usr/bin/wc -l | /usr/bin/tr -d ' '; }

# The lease lifecycle needs pack to get past the network step. `--here` is the honest way: it is a
# real user-facing flag meaning "the network I am on is the commute network", so these cases
# exercise exactly the code path a tethered user takes.
PACK_ARGS=(--here)
if [ -n "$(gateway)" ]; then
    LIFECYCLE_READY=yes
else
    LIFECYCLE_READY=no
fi

echo
echo "rucksack end-to-end test"
echo "binary:  $RUCKSACK"
echo "gateway: $(gateway)"
echo

# --------------------------------------------------------------- read-only commands

new_home
OUT=$("$RUCKSACK" --help 2>&1)
[ $? -eq 0 ] && contains "$OUT" "pack" && [ "$(sleep_disabled)" = "0" ]
check $? "--help works and takes no lease"

for GONE in setup doctor report recover adapters hook; do
    contains "$OUT" "
  $GONE " && fail "$GONE is still in --help" || ok "$GONE is gone from --help"
done

OUT=$("$RUCKSACK" status 2>&1)
[ $? -eq 0 ] && contains "$OUT" "Not packed" && [ "$(lines "$OUT")" -le 2 ]
check $? "status on a fresh Mac is one line" "$OUT"

OUT=$("$RUCKSACK" unpack 2>&1)
[ $? -eq 0 ] && contains "$OUT" "sleeps normally"
check $? "unpack with nothing packed still succeeds" "$OUT"

# --------------------------------------------------------------- arguments

new_home
for ARGS in "--hotspot Noah --usb" "--for 0m" "--for 48h" "--for soon"; do
    # shellcheck disable=SC2086
    "$RUCKSACK" pack $ARGS >/dev/null 2>&1
    [ $? -eq 2 ]
    check $? "pack $ARGS is refused"
done
[ "$(sleep_disabled)" = "0" ]; check $? "refused arguments took no lease"

# --------------------------------------------------------------- broken state recovers

new_home
STATE="$HOME/Library/Application Support/Rucksack"
mkdir -p "$STATE"
printf '{ not json' >"$STATE/session.json"
OUT=$("$RUCKSACK" status 2>&1)
[ $? -eq 0 ] && contains "$OUT" "Not packed"
check $? "status survives unreadable session state" "$OUT"
OUT=$("$RUCKSACK" unpack 2>&1)
[ $? -eq 0 ] && [ ! -f "$STATE/session.json" ]
check $? "unpack clears unreadable session state instead of dead-ending" "$OUT"

new_home
STATE="$HOME/Library/Application Support/Rucksack"
mkdir -p "$STATE"
cat >"$STATE/config.toml" <<'LEGACY'
version = 1
default_agent = "codex"

[hotspot]
ssid = "Noah"
require_verified_ssid = true
probe_timeout_seconds = 6

[session]
duration_minutes = 1440
focus = "continue"
idle_grace_seconds = 900

[safety]
minimum_start_battery_percent = 35
warn_battery_percent = 20
sleep_battery_percent = 15
LEGACY
OUT=$("$RUCKSACK" status 2>&1)
[ $? -eq 0 ] && ! contains "$OUT" "unknown field"
check $? "a config from an older release still loads" "$OUT"

# --------------------------------------------------------------- never pack the network you leave

new_home
# This Mac is on ordinary Wi-Fi, so pack must refuse to accept it and wait instead.
if [ "$(gateway)" != "" ] && [ "$(gateway)" != "172.20.10.1" ]; then
    "$RUCKSACK" pack --for 20m >"$WORK/wait.log" 2>&1 &
    WAIT_PID=$!
    for _ in $(seq 1 30); do
        grep -q "Waiting" "$WORK/wait.log" 2>/dev/null && break
        sleep 1
    done
    OUT=$(cat "$WORK/wait.log")
    contains "$OUT" "Waiting"
    check $? "pack refuses to accept the current network and waits" "$OUT"
    kill -0 "$WAIT_PID" 2>/dev/null
    check $? "pack is still running rather than aborting"
    ! contains "$OUT" "Packed."
    check $? "pack never claimed success on the network being left" "$OUT"
    [ "$(sleep_disabled)" = "0" ]
    check $? "no lease is taken before the network is ready"
    kill "$WAIT_PID" 2>/dev/null
    wait "$WAIT_PID" 2>/dev/null
    sleep 1
    [ "$(sleep_disabled)" = "0" ]
    check $? "interrupting the wait leaves sleep normal"
    pkill -f "System Settings" >/dev/null 2>&1
else
    skip "pack-must-wait (this Mac is already on a hotspot, or offline)"
fi

# --------------------------------------------------------------- the lease lifecycle

new_home
if [ "$LIFECYCLE_READY" = "no" ]; then
    skip "lease lifecycle (this Mac has no working network)"
    skip "the lease keeps standing on its own"
    skip "packing twice refuses"
    skip "unpack restores sleep"
else
    START=$(python3 -c 'import time; print(time.time())')
    OUT=$("$RUCKSACK" pack "${PACK_ARGS[@]}" --for 20m 2>&1)
    CODE=$?
    ELAPSED=$(python3 -c "import time; print(int((time.time() - $START) * 1000))")
    [ $CODE -eq 0 ] && contains "$OUT" "Close the lid and go"
    check $? "pack succeeds with no questions" "$OUT"
    [ "$(lines "$OUT")" -le 5 ]
    check $? "pack prints at most five lines (got $(lines "$OUT"))" "$OUT"
    [ "$(sleep_disabled)" = "1" ]
    check $? "pack actually switched sleep off"
    [ "$ELAPSED" -lt 30000 ]
    check $? "pack finished promptly (${ELAPSED}ms)"

    OUT=$("$RUCKSACK" status 2>&1)
    contains "$OUT" "Packed" && [ "$(lines "$OUT")" -le 2 ]
    check $? "status is one line and says Packed" "$OUT"

    # The whole point of a host-scoped lease: nothing an agent does can end it. There is no longer
    # any agent-facing mechanism at all, so this checks the lease simply keeps standing.
    sleep 3
    OUT=$("$RUCKSACK" status 2>&1)
    contains "$OUT" "Packed" && [ "$(sleep_disabled)" = "1" ]
    check $? "the lease keeps standing on its own, with no agent involvement" "$OUT"

    OUT=$("$RUCKSACK" pack 2>&1)
    [ $? -ne 0 ] && contains "$OUT" "Already packed"
    check $? "packing twice refuses clearly" "$OUT"
    [ "$(sleep_disabled)" = "1" ]
    check $? "the refused second pack left the lease intact"

    OUT=$("$RUCKSACK" unpack 2>&1)
    [ $? -eq 0 ] && contains "$OUT" "sleeps normally"
    check $? "unpack succeeds and says so" "$OUT"
    [ "$(lines "$OUT")" -le 3 ]
    check $? "unpack is at most three lines" "$OUT"
    [ "$(sleep_disabled)" = "0" ]
    check $? "unpack restored normal sleep"
fi

# --------------------------------------------------------------- the skill

new_home
mkdir -p "$HOME/.agents/skills/commute-mode" "$HOME/.claude/skills"
printf '<!-- rucksack-managed -->\nlegacy\n' >"$HOME/.agents/skills/commute-mode/SKILL.md"
if [ "$LIFECYCLE_READY" = "no" ]; then
    skip "skill install (needs a successful pack)"
else
    "$RUCKSACK" pack "${PACK_ARGS[@]}" --for 20m >/dev/null 2>&1
    [ -f "$HOME/.agents/skills/rucksack/SKILL.md" ] \
        && [ ! -d "$HOME/.agents/skills/commute-mode" ]
    check $? "pack installs the rucksack skill and retires commute-mode"
    grep -q "name: rucksack" "$HOME/.agents/skills/rucksack/SKILL.md"
    check $? "the installed skill is named rucksack"
    "$RUCKSACK" unpack >/dev/null 2>&1
fi

# --------------------------------------------------------------- summary

echo
printf '%d passed' "$PASSED"
[ "$SKIPPED" -gt 0 ] && printf ', %d skipped' "$SKIPPED"
if [ "$FAILED" -eq 0 ]; then
    printf ', \033[32m0 failed\033[0m\n'
else
    printf ', \033[31m%d failed\033[0m\n' "$FAILED"
fi
echo
[ "$FAILED" -eq 0 ]
