#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This script runs GUI scenarios: the real Evolution under Xvfb against
# jmap-mockd, driven through AT-SPI. A scenario asserts on the server state
# and on the log, not on pixels. See docs/gui-scenario-tests.md.
#
# Usage: ci/gui-scenario/run.sh [--host] SCENARIO...
# SCENARIO is a file name in ci/gui-scenario/scenarios/, without ".py".
#
# Without --host, the script runs in a rootless podman container with the
# Evolution 3.52 of Ubuntu 24.04 (ci/gui-scenario/Containerfile). It installs
# the components of the host build tree ($GUI_SCENARIO_BUILD, default
# build/) into the container. Build that tree first, on Ubuntu 24.04.
# With --host, the script runs on this machine. Use --host only in a
# throwaway machine or container: it needs the components installed as root.
#
# Each scenario writes to $GUI_SCENARIO_ARTIFACTS/<scenario>/ (default
# build/gui-scenario/): numbered screenshots of the key steps, the logs of
# Evolution, of the D-Bus services and of the mock, and the scenario
# transcript. A failed scenario also leaves the AT-SPI tree and a recording.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCENARIOS="$ROOT/ci/gui-scenario/scenarios"
BUILD="${GUI_SCENARIO_BUILD:-$ROOT/build}"
ARTIFACTS="${GUI_SCENARIO_ARTIFACTS:-$ROOT/build/gui-scenario}"
IMAGE="${GUI_SCENARIO_IMAGE:-localhost/evolution-jmap-gui-scenario}"
COMPONENTS=(book-backend cal-backend camel-provider collection-backend config-module)

usage() {
	echo "usage: ci/gui-scenario/run.sh [--host] SCENARIO..." >&2
	echo "scenarios: $(cd "$SCENARIOS" && find . -name '*.py' -printf '%f ' | sed 's/\.py//g')" >&2
	exit 2
}

host=0
if [ "${1-}" = "--host" ]; then
	host=1
	shift
fi
[ "$#" -gt 0 ] || usage
for scenario in "$@"; do
	[ -f "$SCENARIOS/$scenario.py" ] || usage
done

if [ "$host" -eq 0 ]; then
	mock="$BUILD/cargo-target/release/jmap-mockd"
	if [ ! -x "$mock" ]; then
		echo "gui-scenario: no jmap-mockd at $mock. Build the tree first." >&2
		exit 2
	fi
	if ! podman image exists "$IMAGE"; then
		echo "gui-scenario: building image $IMAGE"
		podman build -f "$ROOT/ci/gui-scenario/Containerfile" -t "$IMAGE" "$ROOT/ci" || exit 2
	fi
	stage="$(mktemp -d)"
	trap 'rm -rf "$stage"' EXIT
	for component in "${COMPONENTS[@]}"; do
		DESTDIR="$stage" cmake --install "$BUILD" --component "$component" >/dev/null || exit 2
	done
	mkdir -p "$ARTIFACTS"
	# The container runs as root, so that it can install the stage. The trap
	# gives the artifacts back to the user of the host.
	podman run --rm --userns=keep-id -u 0 \
		-v "$ROOT:$ROOT:Z" \
		-v "$stage:/stage:Z" \
		-v "$ARTIFACTS:$ARTIFACTS:Z" \
		-w "$ROOT" \
		-e HOST_UID="$(id -u)" \
		-e HOST_GID="$(id -g)" \
		-e GUI_SCENARIO_ARTIFACTS="$ARTIFACTS" \
		-e JMAP_MOCKD="$mock" \
		"$IMAGE" \
		bash -c '
			trap '\''chown -R "$HOST_UID:$HOST_GID" "$GUI_SCENARIO_ARTIFACTS"'\'' EXIT
			cp -a /stage/. /
			ci/gui-scenario/run.sh --host "$@"
		' run.sh "$@"
	exit $?
fi

MOCK_BIN="${JMAP_MOCKD:-$BUILD/cargo-target/release/jmap-mockd}"
DISPLAY_NUM="${GUI_SCENARIO_DISPLAY:-:96}"
MOCK_PORT="${GUI_SCENARIO_PORT:-8090}"
WORK="$(mktemp -d)"

PIDS=()
cleanup() {
	local pid
	for pid in "${PIDS[@]}"; do
		kill "$pid" 2>/dev/null
	done
	wait 2>/dev/null
	PIDS=()
}
trap 'cleanup; rm -rf "$WORK"' EXIT

dump_atspi_tree() {
	python3 -c '
import pyatspi

def walk(acc, depth=0):
    if acc is None or depth > 25:
        return
    try:
        print("  " * depth + f"{acc.getRoleName()}: {acc.name!r}")
    except Exception as error:
        print("  " * depth + f"<error {error}>")
        return
    for i in range(acc.childCount):
        walk(acc.getChildAtIndex(i), depth + 1)

desktop = pyatspi.Registry.getDesktop(0)
for i in range(desktop.childCount):
    walk(desktop.getChildAtIndex(i))
'
}

# Run one scenario in a new profile, display, D-Bus session and mock.
run_scenario() {
	local name="$1"
	local out="$ARTIFACTS/$name"
	local run_dir="$WORK/$name"
	rm -rf "$out"
	mkdir -p "$out" "$run_dir"/{home,config/evolution/sources,data,cache,runtime}
	chmod 700 "$run_dir/runtime"

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
		NO_AT_BRIDGE=0
		EVOLUTION_JMAP_LOG=trace
		PYTHONPATH="$ROOT/ci/gui-scenario"
		GUI_SCENARIO_ARTIFACTS="$out"
		GUI_SCENARIO_SOURCES="$run_dir/config/evolution/sources"
		GUI_SCENARIO_MOCK_PORT="$MOCK_PORT"
		GUI_SCENARIO_JMAP_URL="http://127.0.0.1:$MOCK_PORT"
	)

	"$MOCK_BIN" --port "$MOCK_PORT" >"$out/mock.log" 2>&1 &
	PIDS+=($!)
	Xvfb "$DISPLAY_NUM" -screen 0 1280x1024x24 >"$run_dir/xvfb.log" 2>&1 &
	PIDS+=($!)

	# The setup step writes the sources and the server state before Evolution
	# starts. It also waits for the mock and the display.
	local verdict=0
	"${session_env[@]}" python3 "$SCENARIOS/$name.py" setup \
		>"$out/transcript.txt" 2>&1 || verdict=$?

	if [ "$verdict" -eq 0 ]; then
		"${session_env[@]}" dbus-daemon --session --fork \
			--print-address=3 --print-pid=4 \
			3>"$run_dir/dbus-address" 4>"$run_dir/dbus-pid" 2>"$out/dbus.log"
		PIDS+=("$(cat "$run_dir/dbus-pid")")
		session_env+=(DBUS_SESSION_BUS_ADDRESS="$(cat "$run_dir/dbus-address")")

		# at-spi2-registryd synthesizes input on the display, so the
		# activation environment needs DISPLAY. A window manager gives the
		# windows a position and the keyboard focus.
		"${session_env[@]}" dbus-update-activation-environment DISPLAY EVOLUTION_JMAP_LOG
		"${session_env[@]}" gsettings set org.gnome.desktop.interface toolkit-accessibility true
		# A maximized window shows the full folder tree and message list on the
		# screenshots.
		"${session_env[@]}" gsettings set \
			org.gnome.evolution.window:/org/gnome/evolution/shell/window/ maximized true
		"${session_env[@]}" openbox >"$run_dir/openbox.log" 2>&1 &
		PIDS+=($!)

		local recording="$run_dir/recording.mp4"
		ffmpeg -f x11grab -video_size 1280x1024 -framerate 5 -i "$DISPLAY_NUM" \
			-y "$recording" >"$run_dir/ffmpeg.log" 2>&1 &
		local rec_pid=$!

		"${session_env[@]}" evolution -c mail --force-online >"$out/evolution.log" 2>&1 &
		PIDS+=($!)

		"${session_env[@]}" python3 "$SCENARIOS/$name.py" run \
			>>"$out/transcript.txt" 2>&1 || verdict=$?

		# SIGINT lets ffmpeg write the index, so the file stays playable.
		kill -INT "$rec_pid" 2>/dev/null
		wait "$rec_pid" 2>/dev/null
		if [ "$verdict" -ne 0 ]; then
			"${session_env[@]}" import -window root "$out/failure.png" 2>/dev/null
			"${session_env[@]}" bash -c "$(declare -f dump_atspi_tree); dump_atspi_tree" \
				>"$out/atspi-tree.txt" 2>&1
			cp "$recording" "$out/recording.mp4" 2>/dev/null
		fi
	fi

	cleanup
	sed 's/^/  /' "$out/transcript.txt"
	return "$verdict"
}

failed=()
for scenario in "$@"; do
	echo "gui-scenario: $scenario"
	if run_scenario "$scenario"; then
		echo "gui-scenario: $scenario passed"
	else
		echo "gui-scenario: $scenario FAILED; see $ARTIFACTS/$scenario"
		failed+=("$scenario")
	fi
done

if [ "${#failed[@]}" -gt 0 ]; then
	echo "gui-scenario: failed: ${failed[*]}"
	exit 1
fi
echo "gui-scenario: all passed"
