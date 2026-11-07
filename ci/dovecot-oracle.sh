#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Manage a local, stock Dovecot IMAP test server in rootless podman.
#
# This is the fixture ROADMAP item 175 names: a plain IMAP account for
# ci/gui-scenario's planned comparison against a JMAP account, so a scenario
# can check that a behaviour (for example, not auto-fetching a vCard photo)
# is Evolution's own, not specific to this provider. No scenario uses it yet;
# that wiring is its own, later claim. Image: ci/dovecot-oracle/Containerfile,
# no TLS, PLAIN auth, Maildir storage, one throwaway account.
#
# Usage:
#   ./ci/dovecot-oracle.sh start
#   source ./ci/dovecot-oracle.sh env
#   ./ci/dovecot-oracle.sh deliver <path/to/message.eml>
#   ./ci/dovecot-oracle.sh status
#   ./ci/dovecot-oracle.sh stop

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

DOVECOT_ORACLE_IMAGE="${DOVECOT_ORACLE_IMAGE:-localhost/evolution-jmap-dovecot-oracle}"
DOVECOT_ORACLE_NAME="${DOVECOT_ORACLE_NAME:-evolution-jmap-dovecot-oracle}"
DOVECOT_ORACLE_HOST="${DOVECOT_ORACLE_HOST:-127.0.0.1}"
DOVECOT_ORACLE_IMAP_PORT="${DOVECOT_ORACLE_IMAP_PORT:-8143}"
DOVECOT_ORACLE_USER="${DOVECOT_ORACLE_USER:-tester}"
DOVECOT_ORACLE_PASSWORD="${DOVECOT_ORACLE_PASSWORD:-password}"

_dovecot_oracle_is_sourced() {
	[[ "${BASH_SOURCE[0]}" != "${0}" ]]
}

_dovecot_oracle_usage() {
	cat >&2 <<'EOF'
Usage: ci/dovecot-oracle.sh <start|stop|status|env|deliver> [args...]

Subcommands:
  start                Build the image if needed, start the container, wait until it accepts logins.
  stop                 Stop and remove the container.
  status               Check whether the container is running and accepts logins.
  env                  Print or export JMAP_LIVE_SERVER_IMAP_* environment variables.
  deliver <path>       Drop one RFC 5322 message into the account's INBOX.
EOF
}

_dovecot_oracle_require_tool() {
	local tool="$1"
	if ! command -v "$tool" >/dev/null 2>&1; then
		echo "error: missing required tool: $tool" >&2
		return 1
	fi
}

_dovecot_oracle_export_env() {
	export JMAP_LIVE_SERVER_IMAP_HOST="${DOVECOT_ORACLE_HOST}"
	export JMAP_LIVE_SERVER_IMAP_PORT="${DOVECOT_ORACLE_IMAP_PORT}"
	export JMAP_LIVE_SERVER_IMAP_USER="${DOVECOT_ORACLE_USER}"
	export JMAP_LIVE_SERVER_IMAP_PASSWORD="${DOVECOT_ORACLE_PASSWORD}"

	cat <<EOF
export JMAP_LIVE_SERVER_IMAP_HOST="${JMAP_LIVE_SERVER_IMAP_HOST}"
export JMAP_LIVE_SERVER_IMAP_PORT="${JMAP_LIVE_SERVER_IMAP_PORT}"
export JMAP_LIVE_SERVER_IMAP_USER="${JMAP_LIVE_SERVER_IMAP_USER}"
export JMAP_LIVE_SERVER_IMAP_PASSWORD="${JMAP_LIVE_SERVER_IMAP_PASSWORD}"
EOF
}

_dovecot_oracle_wait_until_ready() {
	local i
	for i in $(seq 1 60); do
		if (exec 3<>"/dev/tcp/${DOVECOT_ORACLE_HOST}/${DOVECOT_ORACLE_IMAP_PORT}") 2>/dev/null; then
			exec 3>&-
			return 0
		fi
		sleep 1
	done
	return 1
}

_dovecot_oracle_status() {
	_dovecot_oracle_require_tool podman || return 1
	if ! podman container inspect "${DOVECOT_ORACLE_NAME}" --format '{{.State.Running}}' 2>/dev/null | grep -q '^true$'; then
		echo "dovecot-oracle: container ${DOVECOT_ORACLE_NAME} is not running" >&2
		return 1
	fi
	if ! _dovecot_oracle_wait_until_ready; then
		echo "dovecot-oracle: IMAP port is not responding" >&2
		return 1
	fi
	echo "dovecot-oracle: running at ${DOVECOT_ORACLE_HOST}:${DOVECOT_ORACLE_IMAP_PORT}"
}

_dovecot_oracle_stop() {
	_dovecot_oracle_require_tool podman || return 1
	if podman container exists "${DOVECOT_ORACLE_NAME}" 2>/dev/null; then
		podman stop -t 1 "${DOVECOT_ORACLE_NAME}" >/dev/null 2>&1 || true
		podman rm -f "${DOVECOT_ORACLE_NAME}" >/dev/null 2>&1 || true
	fi
}

_dovecot_oracle_start() {
	_dovecot_oracle_require_tool podman || return 1

	if ! podman image exists "${DOVECOT_ORACLE_IMAGE}"; then
		podman build -t "${DOVECOT_ORACLE_IMAGE}" "$ROOT/ci/dovecot-oracle" || return 1
	fi

	if ! podman container inspect "${DOVECOT_ORACLE_NAME}" --format '{{.State.Running}}' 2>/dev/null | grep -q '^true$'; then
		podman rm -f "${DOVECOT_ORACLE_NAME}" >/dev/null 2>&1 || true
		podman run -d --name "${DOVECOT_ORACLE_NAME}" \
			-p "${DOVECOT_ORACLE_HOST}:${DOVECOT_ORACLE_IMAP_PORT}:143" \
			-e "DOVECOT_ORACLE_USER=${DOVECOT_ORACLE_USER}" \
			-e "DOVECOT_ORACLE_PASSWORD=${DOVECOT_ORACLE_PASSWORD}" \
			"${DOVECOT_ORACLE_IMAGE}" >/dev/null || return 1
	fi

	if ! _dovecot_oracle_wait_until_ready; then
		echo "error: Dovecot container ${DOVECOT_ORACLE_NAME} did not become ready within 60s" >&2
		return 1
	fi

	_dovecot_oracle_export_env
}

_dovecot_oracle_deliver() {
	_dovecot_oracle_require_tool podman || return 1
	local source="$1"
	[ -f "$source" ] || {
		echo "error: no such file: $source" >&2
		return 1
	}
	# Maildir delivery needs no running protocol: a uniquely named file in
	# new/ is a message as soon as it lands. The name only has to be unique
	# in this Maildir; dovecot does not read it.
	local name
	name="$(date +%s).$$.dovecot-oracle"
	podman cp "$source" "${DOVECOT_ORACLE_NAME}:/home/${DOVECOT_ORACLE_USER}/Maildir/new/${name}" || return 1
	podman exec "${DOVECOT_ORACLE_NAME}" chown "${DOVECOT_ORACLE_USER}:${DOVECOT_ORACLE_USER}" \
		"/home/${DOVECOT_ORACLE_USER}/Maildir/new/${name}" || return 1
}

_dovecot_oracle_main() {
	local cmd="${1:-}"
	if [ -z "$cmd" ]; then
		if _dovecot_oracle_is_sourced; then
			cmd="env"
		else
			_dovecot_oracle_usage
			return 1
		fi
	else
		shift
	fi

	case "$cmd" in
	start)
		_dovecot_oracle_start "$@"
		;;
	stop)
		_dovecot_oracle_stop "$@"
		;;
	status)
		_dovecot_oracle_status "$@"
		;;
	env)
		_dovecot_oracle_export_env
		;;
	deliver)
		_dovecot_oracle_deliver "$@"
		;;
	-h | --help | help)
		_dovecot_oracle_usage
		;;
	*)
		echo "error: unknown subcommand: $cmd" >&2
		_dovecot_oracle_usage
		return 1
		;;
	esac
}

if _dovecot_oracle_is_sourced; then
	_dovecot_oracle_main "$@"
else
	set -euo pipefail
	_dovecot_oracle_main "$@"
fi
