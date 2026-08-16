#!/bin/bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# M9 Tier 2: launch Evolution under Xvfb with a JMAP mail account against
# jmap-mockd, and assert via AT-SPI (ci/gui-smoke-assert.py) that the account
# appears and its inbox is non-empty. A canary, not coverage — see
# docs/gui-smoke-test.md. Retries once; a green run leaves no artifacts, a
# failing one leaves a screenshot, a recording of the session, the AT-SPI
# tree and both processes' logs under $GUI_SMOKE_ARTIFACTS.
#
# Requires: evolution, Xvfb, dbus-daemon, python3-pyatspi, imagemagick
# (`import`), ffmpeg, and a built jmap-mockd + libcameljmap.so/.urls installed
# where Camel scans (see docs/gui-smoke-test.md for both).

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="${GUI_SMOKE_WORKDIR:-$(mktemp -d)}"
ARTIFACTS="${GUI_SMOKE_ARTIFACTS:-$WORK/artifacts}"
DISPLAY_NUM="${GUI_SMOKE_DISPLAY:-:97}"
MOCK_PORT="${GUI_SMOKE_PORT:-8080}"
MOCK_BIN="${JMAP_MOCKD:-$ROOT/build/cargo-target/release/jmap-mockd}"

if [ ! -x "$MOCK_BIN" ]; then
	echo "gui-smoke: jmap-mockd not found or not executable at $MOCK_BIN" >&2
	exit 2
fi

mkdir -p "$ARTIFACTS"

RECORDING_ROOT="${GUI_SMOKE_RECORDING_ROOT:-/dev/shm}"

MOCK_PID=""
XVFB_PID=""
DBUS_PID=""
EVO_PID=""
REC_PID=""

cleanup() {
	set +e
	[ -n "$EVO_PID" ] && kill "$EVO_PID" 2>/dev/null
	[ -n "$REC_PID" ] && kill -INT "$REC_PID" 2>/dev/null
	[ -n "$DBUS_PID" ] && kill "$DBUS_PID" 2>/dev/null
	[ -n "$XVFB_PID" ] && kill "$XVFB_PID" 2>/dev/null
	[ -n "$MOCK_PID" ] && kill "$MOCK_PID" 2>/dev/null
	wait 2>/dev/null
	rm -f "$RECORDING_ROOT/gui-smoke-$$"-*.mp4
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

	local recording="$RECORDING_ROOT/gui-smoke-$$-$n.mp4"
	ffmpeg -f x11grab -video_size 1280x1024 -framerate 10 -i "$DISPLAY_NUM" \
		-y "$recording" >"$run_dir/ffmpeg.log" 2>&1 &
	REC_PID=$!

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
	"${session_env[@]}" python3 "$ROOT/ci/gui-smoke-assert.py" || verdict=$?

	# SIGINT (not SIGKILL) lets ffmpeg flush the mp4 moov atom so the file
	# it leaves behind is playable rather than truncated.
	kill -INT "$REC_PID" 2>/dev/null
	wait "$REC_PID" 2>/dev/null
	REC_PID=""

	if [ "$verdict" -ne 0 ]; then
		"${session_env[@]}" import -window root "$ARTIFACTS/screenshot.png" 2>/dev/null
		cp "$recording" "$ARTIFACTS/recording.mp4" 2>/dev/null
		"${session_env[@]}" python3 -c '
import pyatspi

def walk(acc, depth=0, max_depth=10):
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
	rm -f "$recording"

	return "$verdict"
}

if run_attempt 1; then
	echo "gui-smoke: passed on the first attempt"
	exit 0
fi

echo "gui-smoke: first attempt failed, retrying once"
if run_attempt 2; then
	echo "gui-smoke: passed on the retry"
	exit 0
fi

echo "gui-smoke: failed twice; see $ARTIFACTS"
exit 1
