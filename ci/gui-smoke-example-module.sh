#!/bin/bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Item 74's live-Evolution caveat: the packaged example-module's merged
# mail-message-menu item must actually appear, not merely link, under a real
# Evolution shell. Same harness as ci/gui-smoke.sh (private Xvfb/D-Bus/XDG
# tree against jmap-mockd), a different assertion
# (ci/gui-smoke-example-module-assert.py) and no shared state with it: see
# docs/gui-smoke-test.md, which keeps each caveat its own small script
# rather than growing the one canary.
#
# Requires everything ci/gui-smoke.sh requires, plus example-module
# installed where Evolution scans modules (`cmake --install build`, no
# --component filter, so the Unspecified-component example-module target is
# included alongside camel-provider).

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="${GUI_SMOKE_WORKDIR:-$(mktemp -d)}"
ARTIFACTS="${GUI_SMOKE_ARTIFACTS:-$WORK/artifacts}"
DISPLAY_NUM="${GUI_SMOKE_DISPLAY:-:98}"
MOCK_PORT="${GUI_SMOKE_PORT:-8081}"
MOCK_BIN="${JMAP_MOCKD:-$ROOT/build/cargo-target/release/jmap-mockd}"

if [ ! -x "$MOCK_BIN" ]; then
	echo "gui-smoke-example-module: jmap-mockd not found or not executable at $MOCK_BIN" >&2
	exit 2
fi

mkdir -p "$ARTIFACTS"

MOCK_PID=""
XVFB_PID=""
DBUS_PID=""
EVO_PID=""

cleanup() {
	set +e
	[ -n "$EVO_PID" ] && kill "$EVO_PID" 2>/dev/null
	[ -n "$DBUS_PID" ] && kill "$DBUS_PID" 2>/dev/null
	[ -n "$XVFB_PID" ] && kill "$XVFB_PID" 2>/dev/null
	[ -n "$MOCK_PID" ] && kill "$MOCK_PID" 2>/dev/null
	wait 2>/dev/null
}
trap cleanup EXIT

run_attempt() {
	local n="$1"
	local run_dir="$WORK/attempt-$n"
	rm -rf "$run_dir"
	mkdir -p "$run_dir"/{home,config/evolution/sources,data,cache,runtime}
	chmod 700 "$run_dir/runtime"

	cp "$ROOT/docs/examples/jmap-mock-standalone-mail.source" "$run_dir/config/evolution/sources/"
	cp "$ROOT/docs/examples/jmap-mock-standalone-identity.source" "$run_dir/config/evolution/sources/"
	cp "$ROOT/docs/examples/jmap-mock-standalone-transport.source" "$run_dir/config/evolution/sources/"

	"$MOCK_BIN" --port "$MOCK_PORT" >"$run_dir/mock.log" 2>&1 &
	MOCK_PID=$!

	Xvfb "$DISPLAY_NUM" -screen 0 1280x1024x24 >"$run_dir/xvfb.log" 2>&1 &
	XVFB_PID=$!
	sleep 1

	local session_env=(
		env -i
		PATH="$PATH"
		HOME="$run_dir/home"
		XDG_CONFIG_HOME="$run_dir/config"
		XDG_DATA_HOME="$run_dir/data"
		XDG_CACHE_HOME="$run_dir/cache"
		XDG_RUNTIME_DIR="$run_dir/runtime"
		DISPLAY="$DISPLAY_NUM"
		LANG=C
		LC_ALL=C
	)

	"${session_env[@]}" dbus-daemon --session --fork \
		--print-address=1 --print-pid=2 \
		1>"$run_dir/dbus-address" 2>"$run_dir/dbus-pid"
	DBUS_PID="$(cat "$run_dir/dbus-pid")"
	session_env+=(DBUS_SESSION_BUS_ADDRESS="$(cat "$run_dir/dbus-address")")

	"${session_env[@]}" gsettings set org.gnome.desktop.interface toolkit-accessibility true

	"${session_env[@]}" evolution -c mail --force-online >"$run_dir/evolution.log" 2>&1 &
	EVO_PID=$!

	local verdict=0
	"${session_env[@]}" python3 "$ROOT/ci/gui-smoke-example-module-assert.py" || verdict=$?

	if [ "$verdict" -ne 0 ]; then
		"${session_env[@]}" import -window root "$ARTIFACTS/screenshot.png" 2>/dev/null
		"${session_env[@]}" python3 -c '
import pyatspi

def walk(acc, depth=0, max_depth=12):
    if acc is None or depth > max_depth:
        return
    try:
        print("  " * depth + f"{acc.getRoleName()}: {acc.name!r}")
    except Exception as error:
        print("  " * depth + f"<error {error}>")
        return
    for i in range(acc.childCount):
        walk(acc.getChildAtIndex(i), depth + 1, max_depth)

desktop = pyatspi.Registry.getDesktop(0)
for i in range(desktop.childCount):
    walk(desktop.getChildAtIndex(i))
' >"$ARTIFACTS/atspi-tree.txt" 2>&1
		cp "$run_dir/evolution.log" "$ARTIFACTS/evolution.log" 2>/dev/null
		cp "$run_dir/mock.log" "$ARTIFACTS/mock.log" 2>/dev/null
		cp "$run_dir/xvfb.log" "$ARTIFACTS/xvfb.log" 2>/dev/null
	fi

	kill "$EVO_PID" 2>/dev/null
	kill "$DBUS_PID" 2>/dev/null
	kill "$XVFB_PID" 2>/dev/null
	kill "$MOCK_PID" 2>/dev/null
	wait "$EVO_PID" "$DBUS_PID" "$XVFB_PID" "$MOCK_PID" 2>/dev/null
	EVO_PID=""
	DBUS_PID=""
	XVFB_PID=""
	MOCK_PID=""

	return "$verdict"
}

if run_attempt 1; then
	echo "gui-smoke-example-module: passed on the first attempt"
	exit 0
fi

echo "gui-smoke-example-module: first attempt failed, retrying once"
if run_attempt 2; then
	echo "gui-smoke-example-module: passed on the retry"
	exit 0
fi

echo "gui-smoke-example-module: failed twice; see $ARTIFACTS"
exit 1
