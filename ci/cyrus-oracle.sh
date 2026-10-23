#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Manage a local Cyrus IMAP JMAP test server in rootless podman.
#
# Image: ghcr.io/cyrusimap/cyrus-docker-test-server
# Index digest: sha256:035aa4ffdb84df95a1057645d05c9a156dde74e5787ef1a11f7d9c244a38b2c0
# Linux amd64 digest: sha256:bda776aca84f35128c152dc157bc49e788d9040bb3c3adec21ba5fd2e530bc55
# Cyrus version: Cyrus JMAP 3.13.8-115-gb2ee17d30
#
# Enabled services and imapd.conf settings:
#   httpmodules: caldav carddav jmap
#   jmap_nonstandard_extensions: yes
#   jmap_require_accountid: yes
#   jmap_querycache_max_age: 0
#   jmapsubmission_deleteonsend: no
#   jmap_set_has_attachment: yes
#   fakesaslauthd: accepts any password for provisioned accounts
#
# Usage:
#   ./ci/cyrus-oracle.sh start
#   source ./ci/cyrus-oracle.sh env
#   ./ci/cyrus-oracle.sh seed <username> [username...]
#   ./ci/cyrus-oracle.sh delete-user <username> [username...]
#   ./ci/cyrus-oracle.sh status
#   ./ci/cyrus-oracle.sh stop

CYRUS_ORACLE_IMAGE="${CYRUS_ORACLE_IMAGE:-ghcr.io/cyrusimap/cyrus-docker-test-server@sha256:035aa4ffdb84df95a1057645d05c9a156dde74e5787ef1a11f7d9c244a38b2c0}"
CYRUS_ORACLE_NAME="${CYRUS_ORACLE_NAME:-evolution-jmap-cyrus-oracle}"
CYRUS_ORACLE_HOST="${CYRUS_ORACLE_HOST:-127.0.0.1}"
CYRUS_ORACLE_HTTP_PORT="${CYRUS_ORACLE_HTTP_PORT:-8080}"
CYRUS_ORACLE_WEB_PORT="${CYRUS_ORACLE_WEB_PORT:-8001}"
CYRUS_ORACLE_IMAP_PORT="${CYRUS_ORACLE_IMAP_PORT:-8143}"
CYRUS_ORACLE_POP3_PORT="${CYRUS_ORACLE_POP3_PORT:-8110}"
CYRUS_ORACLE_LMTP_PORT="${CYRUS_ORACLE_LMTP_PORT:-8024}"
CYRUS_ORACLE_SIEVE_PORT="${CYRUS_ORACLE_SIEVE_PORT:-4190}"
CYRUS_ORACLE_SMTP_PORT="${CYRUS_ORACLE_SMTP_PORT:-8587}"
CYRUS_ORACLE_MAILPIT_PORT="${CYRUS_ORACLE_MAILPIT_PORT:-8025}"
CYRUS_ORACLE_DOMAIN="${CYRUS_ORACLE_DOMAIN:-example.com}"
CYRUS_ORACLE_QUOTA_KB="${CYRUS_ORACLE_QUOTA_KB:-102400}"

CYRUS_ORACLE_USER="${CYRUS_ORACLE_USER:-user1}"
CYRUS_ORACLE_PASSWORD="${CYRUS_ORACLE_PASSWORD:-password}"
CYRUS_ORACLE_WRITE_USER="${CYRUS_ORACLE_WRITE_USER:-writer}"
CYRUS_ORACLE_WRITE_PASSWORD="${CYRUS_ORACLE_WRITE_PASSWORD:-password}"
CYRUS_ORACLE_RECIPIENT_USER="${CYRUS_ORACLE_RECIPIENT_USER:-recipient}"
CYRUS_ORACLE_RECIPIENT_PASSWORD="${CYRUS_ORACLE_RECIPIENT_PASSWORD:-password}"

_cyrus_oracle_is_sourced() {
    [[ "${BASH_SOURCE[0]}" != "${0}" ]]
}

_cyrus_oracle_usage() {
    cat >&2 <<'EOF'
Usage: ci/cyrus-oracle.sh <start|stop|status|env|seed|delete-user> [args...]

Subcommands:
  start                    Start the Cyrus container and seed default test accounts.
  stop                     Stop and remove the Cyrus container.
  status                   Check whether the Cyrus container and JMAP endpoint are ready.
  env                      Print or export JMAP_LIVE_SERVER_* environment variables.
  seed [user...]           Create or reset throwaway accounts with mail, contacts, calendars, and quota.
  delete-user <user...>    Delete throwaway accounts from the running Cyrus server.
EOF
}

_cyrus_oracle_require_tool() {
    local tool="$1"
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "error: missing required tool: $tool" >&2
        return 1
    fi
}

_cyrus_oracle_export_env() {
    export CYRUS_ORACLE_URL="http://${CYRUS_ORACLE_HOST}:${CYRUS_ORACLE_HTTP_PORT}"
    export CYRUS_ORACLE_ADMIN_API="http://${CYRUS_ORACLE_HOST}:${CYRUS_ORACLE_WEB_PORT}"
    export JMAP_LIVE_SERVER_URL="http://${CYRUS_ORACLE_HOST}:${CYRUS_ORACLE_HTTP_PORT}"
    export JMAP_LIVE_SERVER_USER="${CYRUS_ORACLE_USER}"
    export JMAP_LIVE_SERVER_PASSWORD="${CYRUS_ORACLE_PASSWORD}"
    export JMAP_LIVE_SERVER_WRITE_USER="${CYRUS_ORACLE_WRITE_USER}"
    export JMAP_LIVE_SERVER_WRITE_PASSWORD="${CYRUS_ORACLE_WRITE_PASSWORD}"
    export JMAP_LIVE_SERVER_RECIPIENT_USER="${CYRUS_ORACLE_RECIPIENT_USER}"
    export JMAP_LIVE_SERVER_RECIPIENT_PASSWORD="${CYRUS_ORACLE_RECIPIENT_PASSWORD}"

    cat <<EOF
export CYRUS_ORACLE_URL="${CYRUS_ORACLE_URL}"
export CYRUS_ORACLE_ADMIN_API="${CYRUS_ORACLE_ADMIN_API}"
export JMAP_LIVE_SERVER_URL="${JMAP_LIVE_SERVER_URL}"
export JMAP_LIVE_SERVER_USER="${JMAP_LIVE_SERVER_USER}"
export JMAP_LIVE_SERVER_PASSWORD="${JMAP_LIVE_SERVER_PASSWORD}"
export JMAP_LIVE_SERVER_WRITE_USER="${JMAP_LIVE_SERVER_WRITE_USER}"
export JMAP_LIVE_SERVER_WRITE_PASSWORD="${JMAP_LIVE_SERVER_WRITE_PASSWORD}"
export JMAP_LIVE_SERVER_RECIPIENT_USER="${JMAP_LIVE_SERVER_RECIPIENT_USER}"
export JMAP_LIVE_SERVER_RECIPIENT_PASSWORD="${JMAP_LIVE_SERVER_RECIPIENT_PASSWORD}"
EOF
}

_cyrus_oracle_seed_one() {
    local userid="$1"
    local admin_url="http://${CYRUS_ORACLE_HOST}:${CYRUS_ORACLE_WEB_PORT}"
    local jmap_url="http://${CYRUS_ORACLE_HOST}:${CYRUS_ORACLE_HTTP_PORT}/jmap/"
    local payload

    payload=$(cat <<EOF
{
  "mailboxes": [
    {"name": "INBOX", "subscribed": true},
    {"name": "Archive", "subscribed": true, "specialUse": "\\\\Archive"},
    {"name": "Drafts", "subscribed": true, "specialUse": "\\\\Drafts"},
    {"name": "Sent", "subscribed": true, "specialUse": "\\\\Sent"},
    {"name": "Spam", "subscribed": true, "specialUse": "\\\\Junk"},
    {"name": "Trash", "subscribed": true, "specialUse": "\\\\Trash"}
  ],
  "quota_kb": ${CYRUS_ORACLE_QUOTA_KB}
}
EOF
)

    curl -sf -X PUT "${admin_url}/${userid}" \
        -H "Content-Type: application/json" \
        -d "${payload}" >/dev/null

    # Trigger initial JMAP AddressBook/get and Calendar/get so Cyrus provisions default collections.
    curl -sf -u "${userid}:${CYRUS_ORACLE_PASSWORD}" -X POST \
        -H "Content-Type: application/json" \
        -d "{\"using\":[\"urn:ietf:params:jmap:core\",\"urn:ietf:params:jmap:contacts\",\"urn:ietf:params:jmap:calendars\"],\"methodCalls\":[[\"AddressBook/get\",{},\"0\"],[\"Calendar/get\",{},\"1\"]]}" \
        "${jmap_url}" >/dev/null
}

_cyrus_oracle_seed() {
    _cyrus_oracle_require_tool curl || return 1
    local users=("$@")
    if [ "${#users[@]}" -eq 0 ]; then
        users=("${CYRUS_ORACLE_USER}" "${CYRUS_ORACLE_WRITE_USER}" "${CYRUS_ORACLE_RECIPIENT_USER}")
    fi
    local u
    for u in "${users[@]}"; do
        _cyrus_oracle_seed_one "$u" || return 1
    done
}

_cyrus_oracle_delete_users() {
    _cyrus_oracle_require_tool curl || return 1
    if [ "$#" -eq 0 ]; then
        echo "error: delete-user requires at least one username" >&2
        return 1
    fi
    local admin_url="http://${CYRUS_ORACLE_HOST}:${CYRUS_ORACLE_WEB_PORT}"
    local u
    for u in "$@"; do
        curl -sf -X DELETE "${admin_url}/${u}" >/dev/null || return 1
    done
}

_cyrus_oracle_status() {
    _cyrus_oracle_require_tool podman || return 1
    _cyrus_oracle_require_tool curl || return 1
    if ! podman container inspect "${CYRUS_ORACLE_NAME}" --format '{{.State.Running}}' 2>/dev/null | grep -q '^true$'; then
        echo "cyrus-oracle: container ${CYRUS_ORACLE_NAME} is not running" >&2
        return 1
    fi
    if ! curl -sfL -u "${CYRUS_ORACLE_USER}:${CYRUS_ORACLE_PASSWORD}" \
        "http://${CYRUS_ORACLE_HOST}:${CYRUS_ORACLE_HTTP_PORT}/.well-known/jmap" >/dev/null 2>&1; then
        echo "cyrus-oracle: JMAP session endpoint is not responding" >&2
        return 1
    fi
    echo "cyrus-oracle: running at http://${CYRUS_ORACLE_HOST}:${CYRUS_ORACLE_HTTP_PORT}"
}

_cyrus_oracle_stop() {
    _cyrus_oracle_require_tool podman || return 1
    if podman container exists "${CYRUS_ORACLE_NAME}" 2>/dev/null; then
        podman stop -t 1 "${CYRUS_ORACLE_NAME}" >/dev/null 2>&1 || true
        podman rm -f "${CYRUS_ORACLE_NAME}" >/dev/null 2>&1 || true
    fi
}

_cyrus_oracle_start() {
    _cyrus_oracle_require_tool podman || return 1
    _cyrus_oracle_require_tool curl || return 1

    if ! podman container inspect "${CYRUS_ORACLE_NAME}" --format '{{.State.Running}}' 2>/dev/null | grep -q '^true$'; then
        podman rm -f "${CYRUS_ORACLE_NAME}" >/dev/null 2>&1 || true
        podman run -d --name "${CYRUS_ORACLE_NAME}" \
            -p "${CYRUS_ORACLE_HOST}:${CYRUS_ORACLE_HTTP_PORT}:8080" \
            -p "${CYRUS_ORACLE_HOST}:${CYRUS_ORACLE_WEB_PORT}:8001" \
            -p "${CYRUS_ORACLE_HOST}:${CYRUS_ORACLE_IMAP_PORT}:8143" \
            -p "${CYRUS_ORACLE_HOST}:${CYRUS_ORACLE_POP3_PORT}:8110" \
            -p "${CYRUS_ORACLE_HOST}:${CYRUS_ORACLE_LMTP_PORT}:8024" \
            -p "${CYRUS_ORACLE_HOST}:${CYRUS_ORACLE_SIEVE_PORT}:4190" \
            -p "${CYRUS_ORACLE_HOST}:${CYRUS_ORACLE_SMTP_PORT}:8587" \
            -p "${CYRUS_ORACLE_HOST}:${CYRUS_ORACLE_MAILPIT_PORT}:8025" \
            -e "DEFAULTDOMAIN=${CYRUS_ORACLE_DOMAIN}" \
            -e "JMAPBASEURL=http://${CYRUS_ORACLE_HOST}:${CYRUS_ORACLE_HTTP_PORT}" \
            "${CYRUS_ORACLE_IMAGE}" >/dev/null || return 1
    fi

    local ready=0
    local i
    for i in $(seq 1 60); do
        if curl -sf "http://${CYRUS_ORACLE_HOST}:${CYRUS_ORACLE_WEB_PORT}/" >/dev/null 2>&1 && \
           curl -sfL -u "user1:${CYRUS_ORACLE_PASSWORD}" "http://${CYRUS_ORACLE_HOST}:${CYRUS_ORACLE_HTTP_PORT}/.well-known/jmap" >/dev/null 2>&1; then
            ready=1
            break
        fi
        sleep 1
    done

    if [ "$ready" -ne 1 ]; then
        echo "error: Cyrus container ${CYRUS_ORACLE_NAME} did not become ready within 60s" >&2
        return 1
    fi

    _cyrus_oracle_seed "${CYRUS_ORACLE_USER}" "${CYRUS_ORACLE_WRITE_USER}" "${CYRUS_ORACLE_RECIPIENT_USER}" || return 1
    _cyrus_oracle_export_env
}

_cyrus_oracle_main() {
    local cmd="${1:-}"
    if [ -z "$cmd" ]; then
        if _cyrus_oracle_is_sourced; then
            cmd="env"
        else
            _cyrus_oracle_usage
            return 1
        fi
    else
        shift
    fi

    case "$cmd" in
        start)
            _cyrus_oracle_start "$@"
            ;;
        stop)
            _cyrus_oracle_stop "$@"
            ;;
        status)
            _cyrus_oracle_status "$@"
            ;;
        env)
            _cyrus_oracle_export_env
            ;;
        seed)
            _cyrus_oracle_seed "$@"
            ;;
        delete-user)
            _cyrus_oracle_delete_users "$@"
            ;;
        -h|--help|help)
            _cyrus_oracle_usage
            ;;
        *)
            echo "error: unknown subcommand: $cmd" >&2
            _cyrus_oracle_usage
            return 1
            ;;
    esac
}

if _cyrus_oracle_is_sourced; then
    _cyrus_oracle_main "$@"
else
    set -euo pipefail
    _cyrus_oracle_main "$@"
fi
