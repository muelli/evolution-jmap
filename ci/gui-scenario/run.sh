#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This script runs GUI scenarios: the real Evolution under Xvfb against
# jmap-mockd, driven through AT-SPI. A scenario asserts on the server state
# and on the log, not on pixels. See docs/gui-scenario-tests.md.
#
# Usage: ci/gui-scenario/run.sh [--host] [--live] [--locale LOCALE] [--scale PERCENT]
#   [--geometry WxH] SCENARIO...
# SCENARIO is a file name in ci/gui-scenario/scenarios/, without ".py".
#
# --locale sets LANG and LC_ALL for the Evolution session (default: C).
# --scale sets org.gnome.desktop.interface text-scaling-factor to PERCENT/100
# (default: the GNOME default, 100). Both reach the scenario script as
# GUI_SCENARIO_LOCALE and GUI_SCENARIO_SCALE.
# --geometry sets the virtual screen size, as WIDTHxHEIGHT (default:
# 1280x1024). The Evolution window stays maximized, so this is also its own
# size. Reaches the scenario script as GUI_SCENARIO_GEOMETRY.
#
# Without --host, the script runs in a rootless podman container with the
# Evolution 3.56 of Ubuntu 26.04 (ci/gui-scenario/Containerfile). It installs
# the components of the host build tree ($GUI_SCENARIO_BUILD, default
# build/) into the container. Build that tree first, on a host whose
# `evolution-shell-3.0` pkg-config version matches the container's.
# $GUI_SCENARIO_MOCKD selects a different jmap-mockd binary.
# With --host, the script runs on this machine. Use --host only in a
# throwaway machine or container: it needs the components installed as root.
#
# With --live, a scenario runs against a real JMAP server instead of
# jmap-mockd: ci/gui-scenario/live_proxy.py relays 127.0.0.1:$GUI_SCENARIO_PORT
# to it, so the account and scenario.Jmap keep using the same loopback
# address either way. Needs three env vars set by the caller, never written
# to this repo and never logged: GUI_SCENARIO_JMAP_URL (the server's own
# reachable address), GUI_SCENARIO_USER and GUI_SCENARIO_PASSWORD (a seeded
# account, e.g. harness/stalwart/seed-manual-test.py's own --state and
# --domain). GUI_SCENARIO_STATE_FILE, if set, names that --state file for a
# scenario to read ids and names from (see scenario.py's live_state()). A
# --live run joins no podman network of its own: it needs the host's own
# route to the server (the dovecot-oracle bridge in the mock path has none),
# so one scenario runs at a time this way.
#
# A scenario that compares against plain IMAP (item 175) needs
# ci/dovecot-oracle.sh running first: `./ci/dovecot-oracle.sh start`. This
# script always joins the same podman network the oracle does
# (DOVECOT_ORACLE_NETWORK, created here if the oracle has not made it yet),
# so such a scenario reaches the oracle by its container name. A scenario
# that does not need IMAP is unaffected: joining the network costs nothing
# and changes no other scenario's behaviour.
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
COMPONENTS=(book-backend cal-backend camel-provider collection-backend config-module translations)
# Must match ci/dovecot-oracle.sh's own default exactly: the whole point is
# for both containers to land on the one network.
DOVECOT_ORACLE_NETWORK="${DOVECOT_ORACLE_NETWORK:-evolution-jmap-gui-scenario}"

usage() {
	echo "usage: ci/gui-scenario/run.sh [--host] [--live] [--locale LOCALE] [--scale PERCENT] [--geometry WxH] SCENARIO..." >&2
	echo "scenarios: $(cd "$SCENARIOS" && find . -name '*.py' -printf '%f ' | sed 's/\.py//g')" >&2
	exit 2
}

# The host build links against the EUIManager or GtkUIManager API by the
# installed Evolution version (evo-sys/build.rs), so the container needs the
# Evolution of the same major.minor. Pick the Ubuntu base by the host's
# version, and rebuild a cached image that has a different one.
evo_major_minor() { cut -d. -f1,2 <<<"$1"; }

ubuntu_for_evolution() {
	case "$1" in
	3.52) echo "docker.io/library/ubuntu:24.04@sha256:561618e2c15bf2397621dd04f96926663a3b5616c189cf7e38db7e82f5c538ea" ;;
	3.56) echo "docker.io/library/ubuntu:26.04@sha256:0d5255be560e2d2ddcfbfda88549e9e815ef29e7cd7fc62183d5709e6e2963c6" ;;
	*) return 1 ;;
	esac
}

image_evolution() {
	podman run --rm "$1" dpkg-query -W -f='${Version}' evolution-data-server 2>/dev/null
}

prepare_image() {
	local image="$1" host_version wanted base
	host_version="$(pkg-config --modversion evolution-shell-3.0 2>/dev/null)" || {
		echo "gui-scenario: pkg-config has no evolution-shell-3.0 on this host" >&2
		exit 2
	}
	wanted="$(evo_major_minor "$host_version")"
	base="$(ubuntu_for_evolution "$wanted")" || {
		echo "gui-scenario: no Ubuntu base image known for the host's Evolution $host_version; add one to ubuntu_for_evolution()" >&2
		exit 2
	}
	if podman image exists "$image" && [ "$(evo_major_minor "$(image_evolution "$image")")" = "$wanted" ]; then
		return 0
	fi
	echo "gui-scenario: building image $image for Evolution $wanted"
	podman build --build-arg "UBUNTU=$base" -f "$ROOT/ci/gui-scenario/Containerfile" -t "$image" "$ROOT/ci" || exit 2
	if [ "$(evo_major_minor "$(image_evolution "$image")")" != "$wanted" ]; then
		echo "gui-scenario: $image does not have Evolution $wanted after a build from $base" >&2
		exit 2
	fi
}

host=0
live=0
locale=""
scale=""
geometry=""
while :; do
	case "${1-}" in
	--host) host=1 ;;
	--live) live=1 ;;
	--locale)
		shift
		locale="${1-}"
		[ -n "$locale" ] || usage
		;;
	--scale)
		shift
		scale="${1-}"
		[ -n "$scale" ] || usage
		;;
	--geometry)
		shift
		geometry="${1-}"
		case "$geometry" in
		[0-9]*x[0-9]*) ;;
		*) usage ;;
		esac
		;;
	*) break ;;
	esac
	shift
done
[ "$#" -gt 0 ] || usage
for scenario in "$@"; do
	[ -f "$SCENARIOS/$scenario.py" ] || usage
done
if [ "$live" -eq 1 ]; then
	: "${GUI_SCENARIO_JMAP_URL:?gui-scenario: --live needs GUI_SCENARIO_JMAP_URL, the real server's reachable address}"
	: "${GUI_SCENARIO_USER:?gui-scenario: --live needs GUI_SCENARIO_USER, a seeded account's address}"
	: "${GUI_SCENARIO_PASSWORD:?gui-scenario: --live needs GUI_SCENARIO_PASSWORD}"
fi

if [ "$host" -eq 0 ]; then
	mock="${GUI_SCENARIO_MOCKD:-$BUILD/cargo-target/release/jmap-mockd}"
	if [ ! -x "$mock" ]; then
		echo "gui-scenario: no jmap-mockd at $mock. Build the tree first." >&2
		exit 2
	fi
	prepare_image "$IMAGE"
	stage="$(mktemp -d)"
	trap 'rm -rf "$stage"' EXIT
	for component in "${COMPONENTS[@]}"; do
		DESTDIR="$stage" cmake --install "$BUILD" --component "$component" >/dev/null || exit 2
	done
	mkdir -p "$ARTIFACTS"
	if [ "$live" -eq 1 ]; then
		# The real server is rarely on the dovecot-oracle bridge's subnet
		# (it is IPv6-only here, and that bridge has no IPv6 of its own);
		# host networking gives the container the runner's own route.
		network_args=(--network host)
	else
		if ! podman network exists "$DOVECOT_ORACLE_NETWORK" 2>/dev/null; then
			podman network create "$DOVECOT_ORACLE_NETWORK" >/dev/null || exit 2
		fi
		network_args=(--network "$DOVECOT_ORACLE_NETWORK")
	fi
	# A scenario against the IMAP oracle needs delivery done from here, on
	# the host: the inner container below has no podman of its own to reach
	# a sibling container's Maildir. The oracle itself must already be
	# running (`ci/dovecot-oracle.sh start`); this only seeds its mailbox.
	for scenario in "$@"; do
		fixture=""
		case "$scenario" in
		mail-imap-oracle-smoke) fixture="imap-oracle-smoke.eml" ;;
		mail-imap-vcard-photo-fetch) fixture="imap-vcard-photo.eml" ;;
		esac
		if [ -n "$fixture" ]; then
			if ! "$ROOT/ci/dovecot-oracle.sh" status >/dev/null 2>&1; then
				echo "gui-scenario: $scenario needs the IMAP oracle running first:" >&2
				echo "  ci/dovecot-oracle.sh start" >&2
				exit 2
			fi
			"$ROOT/ci/dovecot-oracle.sh" deliver \
				"$ROOT/ci/gui-scenario/fixtures/$fixture" || exit 2
		fi
	done
	live_env_args=()
	live_mount_args=()
	extra_flags=()
	if [ "$live" -eq 1 ]; then
		# Bare -e: podman reads the value from this shell's own
		# environment. Never given a literal value here, so it never
		# appears in this script, in its argv or in any log it writes.
		live_env_args=(-e GUI_SCENARIO_JMAP_URL -e GUI_SCENARIO_USER \
			-e GUI_SCENARIO_PASSWORD -e GUI_SCENARIO_STATE_FILE)
		extra_flags=(--live)
		# The state file is outside $ROOT (it holds a seeded account's
		# password, so it belongs next to the other harness secrets, not
		# in this tree), so the $ROOT bind mount below does not carry it
		# in on its own.
		if [ -n "${GUI_SCENARIO_STATE_FILE:-}" ]; then
			live_mount_args=(-v "$GUI_SCENARIO_STATE_FILE:$GUI_SCENARIO_STATE_FILE:ro")
		fi
	fi
	if [ -n "$locale" ]; then
		extra_flags+=(--locale "$locale")
	fi
	if [ -n "$scale" ]; then
		extra_flags+=(--scale "$scale")
	fi
	if [ -n "$geometry" ]; then
		extra_flags+=(--geometry "$geometry")
	fi
	# The container runs as root, so that it can install the stage. The trap
	# gives the artifacts back to the user of the host.
	podman run --rm --userns=keep-id -u 0 \
		"${network_args[@]}" \
		-v "$ROOT:$ROOT:Z" \
		-v "$stage:/stage:Z" \
		-v "$mock:/usr/local/bin/jmap-mockd:ro" \
		-v "$ARTIFACTS:$ARTIFACTS:Z" \
		"${live_mount_args[@]}" \
		-w "$ROOT" \
		-e HOST_UID="$(id -u)" \
		-e HOST_GID="$(id -g)" \
		-e GUI_SCENARIO_ARTIFACTS="$ARTIFACTS" \
		-e JMAP_MOCKD=/usr/local/bin/jmap-mockd \
		-e JMAP_LIVE_SERVER_IMAP_CONTAINER_HOST \
		-e JMAP_LIVE_SERVER_IMAP_CONTAINER_PORT \
		"${live_env_args[@]}" \
		"$IMAGE" \
		bash -c '
			trap '\''chown -R "$HOST_UID:$HOST_GID" "$GUI_SCENARIO_ARTIFACTS"'\'' EXIT
			# A preserved mode and owner on / makes the bwrap of the glycin image loader fail: its user namespace cannot map them.
			cp -a --no-preserve=ownership,mode /stage/. /
			ci/gui-scenario/run.sh --host "$@"
		' run.sh "${extra_flags[@]}" "$@"
	exit $?
fi

MOCK_BIN="${JMAP_MOCKD:-$BUILD/cargo-target/release/jmap-mockd}"
DISPLAY_NUM="${GUI_SCENARIO_DISPLAY:-:96}"
MOCK_PORT="${GUI_SCENARIO_PORT:-8090}"
WORK="$(mktemp -d)"

LIVE_HOST=""
LIVE_PORT=""
if [ "$live" -eq 1 ]; then
	read -r LIVE_HOST LIVE_PORT < <(python3 -c '
import sys, urllib.parse
u = urllib.parse.urlsplit(sys.argv[1])
print(u.hostname, u.port or (443 if u.scheme == "https" else 80))
' "$GUI_SCENARIO_JMAP_URL")
fi

PIDS=()
# The runtime directory of the scenario that runs now. Only the keyring
# daemon needs it: see reap_keyring.
KEYRING_RUNTIME=""
cleanup() {
	local pid
	for pid in "${PIDS[@]}"; do
		kill "$pid" 2>/dev/null
	done
	wait 2>/dev/null
	PIDS=()
	reap_keyring
}

# Stop the keyring daemon of this scenario.
#
# `--daemonize` detaches it, so it is not in PIDS and it stays alive after the
# session bus stops. Find it by the XDG_RUNTIME_DIR of the scenario, which is
# a new temporary directory that no other process has. jmap-functional's own
# session (rust/crates/jmap-functional/src/lib.rs) does the same.
reap_keyring() {
	local pid environment
	[ -n "$KEYRING_RUNTIME" ] || return 0
	for pid in $(pgrep -x gnome-keyring-daemon 2>/dev/null); do
		environment="$(tr '\0' '\n' <"/proc/$pid/environ" 2>/dev/null)"
		if printf '%s\n' "$environment" | grep -qxF "XDG_RUNTIME_DIR=$KEYRING_RUNTIME"; then
			kill "$pid" 2>/dev/null
		fi
	done
	KEYRING_RUNTIME=""
}
trap 'cleanup; rm -rf "$WORK"' EXIT

dump_atspi_tree() {
	python3 -c '
import pyatspi

def walk(acc, depth=0):
    if acc is None or depth > 25:
        return
    try:
        line = f"{acc.getRoleName()}: {acc.name!r}"
        try:
            text = acc.queryText()
            line += f" text={text.getText(0, text.characterCount)!r}"
        except NotImplementedError:
            pass
        states = sorted(
            pyatspi.STATE_VALUE_TO_NAME.get(int(state), str(state))
            for state in acc.getState().getStates()
        )
        line += f" states={states}"
        print("  " * depth + line)
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
		LANG="${locale:-C}"
		LC_ALL="${locale:-C}"
		NO_AT_BRIDGE=0
		EVOLUTION_JMAP_LOG=trace
		PYTHONPATH="$ROOT/ci/gui-scenario"
		PYTHONDONTWRITEBYTECODE=1
		GUI_SCENARIO_LOCALE="$locale"
		GUI_SCENARIO_SCALE="$scale"
		GUI_SCENARIO_GEOMETRY="${geometry:-1280x1024}"
		GUI_SCENARIO_ARTIFACTS="$out"
		GUI_SCENARIO_SOURCES="$run_dir/config/evolution/sources"
		GUI_SCENARIO_MOCK_PORT="$MOCK_PORT"
		GUI_SCENARIO_JMAP_URL="http://127.0.0.1:$MOCK_PORT"
		GUI_SCENARIO_IMAP_HOST="${JMAP_LIVE_SERVER_IMAP_CONTAINER_HOST:-}"
		GUI_SCENARIO_IMAP_PORT="${JMAP_LIVE_SERVER_IMAP_CONTAINER_PORT:-}"
		GUI_SCENARIO_USER="${GUI_SCENARIO_USER:-}"
		GUI_SCENARIO_PASSWORD="${GUI_SCENARIO_PASSWORD:-}"
		GUI_SCENARIO_STATE_FILE="${GUI_SCENARIO_STATE_FILE:-}"
		JMAP_LIVE_SERVER_REBASE_URLS="$live"
	)

	# The scenario names the Evolution component to start and the arguments of
	# the mock. In --live mode there is no mock to configure, and the config
	# step's own extra lines (jmap-mockd's own arguments) are unused.
	local config component
	mapfile -t config < <("${session_env[@]}" python3 "$SCENARIOS/$name.py" config)
	component="${config[0]:-mail}"

	if [ "$live" -eq 1 ]; then
		python3 "$ROOT/ci/gui-scenario/live_proxy.py" "$MOCK_PORT" "$LIVE_HOST" "$LIVE_PORT" \
			>"$out/mock.log" 2>&1 &
	else
		"$MOCK_BIN" --port "$MOCK_PORT" "${config[@]:1}" >"$out/mock.log" 2>&1 &
	fi
	PIDS+=($!)
	Xvfb "$DISPLAY_NUM" -screen 0 "${geometry:-1280x1024}x24" >"$run_dir/xvfb.log" 2>&1 &
	PIDS+=($!)

	# The setup step writes the sources and the server state before Evolution
	# starts. It also waits for the mock and the display.
	local verdict=0
	"${session_env[@]}" python3 "$SCENARIOS/$name.py" setup \
		>"$out/transcript.txt" 2>&1 || verdict=$?

	if [ "$verdict" -eq 0 ]; then
		# Without --fork, the services that D-Bus starts (the registry and the
		# factories) keep the standard error of the daemon, so their log goes
		# to dbus.log.
		"${session_env[@]}" dbus-daemon --session --nofork --print-address=3 \
			3>"$run_dir/dbus-address" >"$out/dbus.log" 2>&1 &
		PIDS+=($!)
		local tries
		for tries in $(seq 100); do
			[ -s "$run_dir/dbus-address" ] && break
			sleep 0.1
		done
		session_env+=(DBUS_SESSION_BUS_ADDRESS="$(cat "$run_dir/dbus-address")")

		# at-spi2-registryd synthesizes input on the display, so the
		# activation environment needs DISPLAY. A window manager gives the
		# windows a position and the keyboard focus.
		"${session_env[@]}" dbus-update-activation-environment DISPLAY EVOLUTION_JMAP_LOG \
			JMAP_LIVE_SERVER_REBASE_URLS

		# A scenario account with a user name needs a secret store: EDS and
		# Evolution keep a password in `org.freedesktop.secrets`, not in the
		# account file. D-Bus activates gnome-keyring-daemon for that name,
		# but the daemon then shows a GTK unlock prompt, which fails on this
		# display. So unlock a new login keyring with the empty password
		# first, as jmap-functional's session does.
		KEYRING_RUNTIME="$run_dir/runtime"
		if ! "${session_env[@]}" sh -c \
			"printf '\n' | gnome-keyring-daemon --daemonize --unlock --components=secrets" \
			>"$run_dir/keyring.log" 2>&1; then
			{
				echo "gui-scenario: gnome-keyring-daemon --unlock failed."
				echo "gui-scenario: is gnome-keyring installed? EDS asks it for every password."
				cat "$run_dir/keyring.log"
			} >>"$out/transcript.txt"
			verdict=1
		fi

		"${session_env[@]}" gsettings set org.gnome.desktop.interface toolkit-accessibility true
		if [ -n "$scale" ]; then
			"${session_env[@]}" gsettings set org.gnome.desktop.interface \
				text-scaling-factor "$(python3 -c "print($scale / 100)")"
		fi
		# A maximized window shows the full folder tree and message list on the
		# screenshots.
		"${session_env[@]}" gsettings set \
			org.gnome.evolution.window:/org/gnome/evolution/shell/window/ maximized true
		# A scenario's account can be contacts/calendars-only, with no mail
		# account; Evolution then shows its first-run "Welcome" assistant
		# (modules/startup-wizard checks for a mail account specifically),
		# a real top-level window that can swallow a synthetic click meant
		# for the window underneath. No scenario tests that assistant.
		"${session_env[@]}" gsettings set org.gnome.evolution.mail show-startup-wizard false
		if [ "$component" = "calendar" ]; then
			# The event/task/memo editor hides its Categories field by
			# default; show it so a scenario can reach it.
			"${session_env[@]}" gsettings set \
				org.gnome.evolution.calendar editor-show-categories true
		fi
		if [ "$name" = "mail-flags-server-push" ] || [ "$name" = "offline-replay-transport-error" ]; then
			# Evolution marks the message the list cursor rests on as read
			# after a timeout (1.5s by default, mark-seen), on its own, with
			# no explicit user action. mail-flags-server-push asserts on a
			# server-driven read/unread change alone, so that timer would
			# race its own assertions; offline-replay-transport-error's
			# fault proxy breaks the first Email/set call it sees, and an
			# unrelated mark-seen keyword sync must not be the one that
			# consumes it. Turn it off for these scenarios only. Other
			# scenarios check read/unread through an explicit Message/Mark
			# As action and rely on mark-seen staying on.
			"${session_env[@]}" gsettings set org.gnome.evolution.mail mark-seen false
		fi
		if [ "$name" = "mail-vcard-photo-fetch-remote-content" ]; then
			# This scenario measures a sender-supplied vCard photo URL with
			# the user's "Load remote content" setting already on, so it
			# must be set before Evolution starts: flipping it live while a
			# message is open makes Evolution reformat that message on its
			# own, which is not a clean measurement.
			"${session_env[@]}" gsettings set \
				org.gnome.evolution.mail image-loading-policy always
		fi
		"${session_env[@]}" openbox >"$run_dir/openbox.log" 2>&1 &
		PIDS+=($!)

		local recording="$run_dir/recording.mp4"
		ffmpeg -f x11grab -video_size "${geometry:-1280x1024}" -framerate 5 -i "$DISPLAY_NUM" \
			-y "$recording" >"$run_dir/ffmpeg.log" 2>&1 &
		local rec_pid=$!

		"${session_env[@]}" evolution -c "$component" --force-online >"$out/evolution.log" 2>&1 &
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
