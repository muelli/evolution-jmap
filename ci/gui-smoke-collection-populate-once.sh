#!/bin/bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Item 66's live-Evolution caveat: a brand-new JMAP collection account's
# first populate must authenticate ONCE, not twice. Same harness as
# ci/gui-smoke.sh (private Xvfb/D-Bus/XDG tree against jmap-mockd), but a
# collection account rather than the standalone mail source gui-smoke.sh
# drops: the double-populate bug item 66 fixed (`on_account_changed`
# answering its own write) lives in `jmap-backend-collection`, which a
# standalone mail source never exercises.
#
# Contacts and calendars are deliberately OFF on this collection account
# (unlike docs/examples/jmap-mock-mail-collection.source, which is the
# manual-test recipe and enables both): `jmap-backend-collection` auto-creates
# and exports a book/calendar child the moment either is on, and each gets
# opened by its own independent EDS client (evolution-addressbook-factory's
# autocompletion, evolution-calendar-factory's reminder watcher) on its own
# schedule, unrelated to this account's own populate. A first run with both on
# confirmed this empirically: `AddressBook/get`/`Calendar/get` counts were
# noisy and did not agree between two otherwise-identical passes, where
# `Mailbox/get`'s own count (the only backend this profile's one Evolution
# instance ever opens itself) was exactly 2 every time, including against
# ci/gui-smoke.sh's own plain standalone account control (no collection
# backend involved at all) confirming 2, not 1, is the healthy number for a
# single connect. So the fixture here is mail-only, and the four small
# sources below are written directly rather than copied from docs/examples:
# the combination is this test's own, not one of the manual-test recipes
# docs/manual-test-collection-backend.md documents for a human to copy.
#
# The oracle is jmap-mock's own request log: `jmap_mock::dispatch::handle_api`
# prints every POST /jmap body to stdout before this script redirects it to
# mock.log, so counting `Mailbox/get` occurrences there after the account's
# inbox is confirmed non-empty (and a settle period has passed) tells one
# real login (2) apart from the bug's second, self-triggered one (4).
#
# Requires everything ci/gui-smoke.sh requires, plus the collection backend
# (`cmake --install build`, no --component filter needed beyond
# camel-provider, which the collection backend module is part of).

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
