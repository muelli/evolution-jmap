#!/bin/bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This script checks a live Evolution instance. It confirms that a
# brand-new JMAP collection account authenticates exactly once during its
# first populate, not twice.
#
# This script uses the same private Xvfb, D-Bus, and XDG tree against
# jmap-mockd as ci/gui-smoke.sh uses. Unlike ci/gui-smoke.sh, this script
# uses a collection account instead of a standalone mail source. The
# double-populate bug that this script guards against lives in
# jmap-backend-collection: the account's "changed" signal handler,
# on_account_changed, can answer its own write and start a second populate.
# A standalone mail source never runs this code path.
#
# This account has contacts and calendars OFF on purpose.
# docs/examples/jmap-mock-mail-collection.source is the manual-test recipe,
# and it turns both on. jmap-backend-collection creates and exports a book
# or a calendar child as soon as contacts or calendars turns on. Each child
# then opens through its own independent EDS client:
# evolution-addressbook-factory's autocompletion opens the book child, and
# evolution-calendar-factory's reminder watcher opens the calendar child.
# Each client opens its child on its own schedule, unrelated to this
# account's own populate. An earlier run with both settings on confirmed
# this directly: the AddressBook/get and Calendar/get counts varied between
# two otherwise identical passes. In every pass, the Mailbox/get count (the
# only backend that this profile's one Evolution instance opens itself) was
# exactly 2. A control run of ci/gui-smoke.sh's own plain standalone
# account, with no collection backend involved, also gave 2, confirming
# that 2, not 1, is the correct count for one successful connection. For
# this reason, this script's fixture is mail-only. The four small source
# files below are written directly, not copied from docs/examples: this
# combination belongs to this test only. It is not one of the manual-test
# recipes that docs/manual-test-collection-backend.md documents for a human
# to copy.
#
# This script tells one real login apart from the bug's second,
# self-triggered login by counting Mailbox/get calls in jmap-mock's own
# request log. jmap_mock::dispatch::handle_api prints every POST /jmap body
# to stdout before this script redirects that output to mock.log. This
# script counts the Mailbox/get occurrences in that log after the account's
# inbox is confirmed non-empty and after a settle period has passed. One
# real login gives a count of 2. The bug's extra, self-triggered login
# raises the count to 4.
#
# This script requires everything that ci/gui-smoke.sh requires. This
# script also requires the collection backend. Install it with
# `cmake --install build` with no --component filter beyond
# camel-provider, because the collection backend module is part of the
# camel-provider component.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="${GUI_SMOKE_WORKDIR:-$(mktemp -d)}"
ARTIFACTS="${GUI_SMOKE_ARTIFACTS:-$WORK/artifacts}"
DISPLAY_NUM="${GUI_SMOKE_DISPLAY:-:99}"
MOCK_PORT="${GUI_SMOKE_PORT:-8082}"
MOCK_BIN="${JMAP_MOCKD:-$ROOT/build/cargo-target/release/jmap-mockd}"

if [ ! -x "$MOCK_BIN" ]; then
	echo "gui-smoke-collection-populate-once: jmap-mockd not found or not executable at $MOCK_BIN" >&2
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

	local sources="$run_dir/config/evolution/sources"
	cat >"$sources/jmap-mock-mail-only-collection.source" <<-EOF
		[Data Source]
		DisplayName=JMAP mock mail-only account
		Enabled=true

		[Collection]
		BackendName=jmap
		ContactsEnabled=false
		CalendarEnabled=false
		MailEnabled=true

		[Authentication]
		Host=127.0.0.1
		Port=$MOCK_PORT

		[Security]
		Method=none
	EOF
	cat >"$sources/jmap-mock-mail-only-account.source" <<-EOF
		[Data Source]
		DisplayName=JMAP mock mail-only inbox
		Enabled=true
		Parent=jmap-mock-mail-only-collection

		[Mail Account]
		BackendName=jmap
		IdentityUid=jmap-mock-mail-only-identity
	EOF
	cat >"$sources/jmap-mock-mail-only-identity.source" <<-EOF
		[Data Source]
		DisplayName=JMAP mock mail-only identity
		Enabled=true
		Parent=jmap-mock-mail-only-collection

		[Mail Identity]
		Name=JMAP mock user
		Address=alice@example.com

		[Mail Submission]
		TransportUid=jmap-mock-mail-only-transport
	EOF
	cat >"$sources/jmap-mock-mail-only-transport.source" <<-EOF
		[Data Source]
		DisplayName=JMAP mock mail-only transport
		Enabled=true
		Parent=jmap-mock-mail-only-collection

		[Mail Transport]
		BackendName=jmap
	EOF

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
	"${session_env[@]}" GUI_SMOKE_MOCK_LOG="$run_dir/mock.log" \
		python3 "$ROOT/ci/gui-smoke-collection-populate-once-assert.py" || verdict=$?

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
	echo "gui-smoke-collection-populate-once: passed on the first attempt"
	exit 0
fi

echo "gui-smoke-collection-populate-once: first attempt failed, retrying once"
if run_attempt 2; then
	echo "gui-smoke-collection-populate-once: passed on the retry"
	exit 0
fi

echo "gui-smoke-collection-populate-once: failed twice; see $ARTIFACTS"
exit 1
