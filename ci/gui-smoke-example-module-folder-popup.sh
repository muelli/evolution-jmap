#!/bin/bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Item 74's other live-Evolution caveat: the packaged example-module's merged
# mail-folder-popup item must actually appear in the folder tree's real
# context menu, not merely link, under a real Evolution shell. Same harness
# as ci/gui-smoke.sh and ci/gui-smoke-example-module.sh (private
# Xvfb/D-Bus/XDG tree against jmap-mockd), a different assertion
# (ci/gui-smoke-example-module-folder-popup-assert.py) and no shared state
# with either: see docs/gui-smoke-test.md, which keeps each caveat its own
# small script rather than growing the one canary.
#
# Unlike the mail-message-menu item, this one merges into a context menu
# Evolution builds fresh on demand when a folder's popup menu is triggered, so
# this script's assertion actually triggers it (focus the row, send the Menu
# key) via AT-SPI instead of reading the tree statically; see the assertion's
# own docstring for why that is a keyboard trigger rather than a coordinate-
# based mouse right-click.
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
	echo "gui-smoke-example-module-folder-popup: jmap-mockd not found or not executable at $MOCK_BIN" >&2
	exit 2
fi

if ! command -v openbox >/dev/null 2>&1; then
	echo "gui-smoke-example-module-folder-popup: openbox not found (see ci/install-deps-gui-smoke.sh)" >&2
	exit 2
fi

# Camel's own default provider-directory lookup (camel_provider_init's
# camel_util_get_directory_variants against its compiled-in E_DATA_SERVER_PREFIX)
# comes up empty under the rootless-podman Ubuntu recipe this harness falls back
# to when no root is available (see docs/gui-smoke-test.md): every account this
# script configures fails with "No provider available for protocol jmap", not
# because the module is missing or broken, but because that lookup never finds
# the directory it is already installed in. Pointing EDS_CAMEL_PROVIDER_DIR at
# the same directory the OS package's own camel providers live in works around
# it; harmless on a host where the default resolution already works.
CAMEL_PROVIDER_DIR="$(dpkg -L evolution-data-server 2>/dev/null | grep -m1 '/camel-providers/lib' | xargs -r dirname)"

mkdir -p "$ARTIFACTS"

MOCK_PID=""
XVFB_PID=""
DBUS_PID=""
OPENBOX_PID=""
EVO_PID=""

cleanup() {
	set +e
	[ -n "$EVO_PID" ] && kill "$EVO_PID" 2>/dev/null
	[ -n "$OPENBOX_PID" ] && kill "$OPENBOX_PID" 2>/dev/null
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
	[ -n "$CAMEL_PROVIDER_DIR" ] && session_env+=(EDS_CAMEL_PROVIDER_DIR="$CAMEL_PROVIDER_DIR")

	"${session_env[@]}" dbus-daemon --session --fork \
		--print-address=1 --print-pid=2 \
		1>"$run_dir/dbus-address" 2>"$run_dir/dbus-pid"
	DBUS_PID="$(cat "$run_dir/dbus-pid")"
	session_env+=(DBUS_SESSION_BUS_ADDRESS="$(cat "$run_dir/dbus-address")")

	# D-Bus activates at-spi2-registryd (the process that actually owns the
	# X display connection XTestFakeKeyEvent/XTestFakeButtonEvent go through)
	# with its own environment, not the env of whichever client triggers the
	# activation, so DISPLAY has to be pushed into it explicitly or
	# pyatspi.Registry's synthetic input calls silently reach no display.
	"${session_env[@]}" dbus-update-activation-environment DISPLAY

	"${session_env[@]}" gsettings set org.gnome.desktop.interface toolkit-accessibility true

	# A minimal window manager: without one, this Xvfb never assigns Evolution's
	# toplevel an absolute screen position or X input focus, so the assertion's
	# AT-SPI-driven right-click (both the coordinate-based and the keyboard
	# route it falls back to) has nothing to land on.
	"${session_env[@]}" openbox >"$run_dir/openbox.log" 2>&1 &
	OPENBOX_PID=$!
	sleep 1

	"${session_env[@]}" evolution -c mail --force-online >"$run_dir/evolution.log" 2>&1 &
	EVO_PID=$!

	local verdict=0
	"${session_env[@]}" python3 "$ROOT/ci/gui-smoke-example-module-folder-popup-assert.py" || verdict=$?

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
		cp "$run_dir/openbox.log" "$ARTIFACTS/openbox.log" 2>/dev/null
	fi

	kill "$EVO_PID" 2>/dev/null
	kill "$OPENBOX_PID" 2>/dev/null
	kill "$DBUS_PID" 2>/dev/null
	kill "$XVFB_PID" 2>/dev/null
	kill "$MOCK_PID" 2>/dev/null
	wait "$EVO_PID" "$OPENBOX_PID" "$DBUS_PID" "$XVFB_PID" "$MOCK_PID" 2>/dev/null
	EVO_PID=""
	OPENBOX_PID=""
	DBUS_PID=""
	XVFB_PID=""
	MOCK_PID=""

	return "$verdict"
}

if run_attempt 1; then
	echo "gui-smoke-example-module-folder-popup: passed on the first attempt"
	exit 0
fi

echo "gui-smoke-example-module-folder-popup: first attempt failed, retrying once"
if run_attempt 2; then
	echo "gui-smoke-example-module-folder-popup: passed on the retry"
	exit 0
fi

echo "gui-smoke-example-module-folder-popup: failed twice; see $ARTIFACTS"
exit 1
