#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Resolve a .deb's Depends against the dpkg database of the host it was built
# on, and write what was found as JSON for ci/sbom.sh. Run it where the package
# was built: dpkg-shlibdeps derived those Depends from the libraries installed
# there, so that host is the only one that knows which version of each library
# the package was linked against. The release job that assembles the SBOM runs
# on a different machine, where most of these libraries are not installed.
#
# For each Depends clause the first alternative that is installed for the
# native architecture (or "all") and satisfies the clause's version constraint
# is recorded. A clause with no such alternative is an error: the SBOM would
# otherwise name a library version the package was never built against.
#
# Output:
#   {"depends": "<the Depends field, verbatim>",
#    "distro": {"id": ..., "version_id": ..., "codename": ...},
#    "packages": [{"name": ..., "version": ..., "arch": ...}, ...]}
#
# Needs bash, dpkg and awk only: it runs in the pinned image the package is
# built in, which has no jq.
set -euo pipefail

deb="$1"   # path to the built .deb
out="$2"   # output JSON path

native_arch="$(dpkg --print-architecture)"
depends="$(dpkg-deb -f "$deb" Depends)"

# Prints $1 as a JSON string. Escapes what a control field or os-release
# value can plausibly hold (a folded field brings newlines); any other control
# character makes invalid JSON, which ci/sbom.sh's jq then rejects loudly.
json_string() {
    local s="${1//\\/\\\\}"
    s="${s//\"/\\\"}"
    s="${s//$'\n'/\\n}"
    s="${s//$'\r'/\\r}"
    s="${s//$'\t'/\\t}"
    printf '"%s"' "$s"
}

# Prints "version<TAB>arch" of the installed instance of package $1 that is
# native or arch-independent, or nothing.
installed() {
    dpkg-query -W -f '${db:Status-Abbrev}\t${Version}\t${Architecture}\n' "$1" \
        2>/dev/null \
        | awk -F'\t' -v native="$native_arch" \
            '$1 ~ /^ii/ && ($3 == native || $3 == "all") { print $2 "\t" $3; exit }' \
        || true
}

# "name[:archqual] [(op version)]"; a variable, because [[ =~ ]] cannot take
# the literal parentheses inline.
alternative_pattern='^[[:space:]]*([a-z0-9][a-z0-9.+-]*)(:[a-z0-9-]+)?[[:space:]]*(\(([<>=]+)[[:space:]]*([^)[:space:]]+)[[:space:]]*\))?[[:space:]]*$'

packages=""
while IFS= read -r clause; do
    clause="$(sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//' <<<"$clause")"
    [ -n "$clause" ] || continue
    resolved=""
    while IFS= read -r alternative; do
        if [[ ! "$alternative" =~ $alternative_pattern ]]; then
            echo "deb-dependencies: cannot parse '$alternative' in '$clause'" >&2
            exit 1
        fi
        name="${BASH_REMATCH[1]}"
        op="${BASH_REMATCH[4]}"
        wanted="${BASH_REMATCH[5]}"
        found="$(installed "$name")"
        [ -n "$found" ] || continue
        version="${found%%$'\t'*}"
        arch="${found#*$'\t'}"
        if [ -n "$op" ] && ! dpkg --compare-versions "$version" "$op" "$wanted"; then
            continue
        fi
        resolved="{\"name\": $(json_string "$name"), \"version\": $(json_string "$version"), \"arch\": $(json_string "$arch")}"
        break
    done < <(tr '|' '\n' <<<"$clause")
    if [ -z "$resolved" ]; then
        echo "deb-dependencies: nothing installed satisfies '$clause'" >&2
        exit 1
    fi
    packages="${packages:+$packages,
}    $resolved"
done < <(tr ',' '\n' <<<"$depends")

# shellcheck source=/dev/null
. /etc/os-release
{
    printf '{\n  "depends": %s,\n' "$(json_string "$depends")"
    printf '  "distro": {"id": %s, "version_id": %s, "codename": %s},\n' \
        "$(json_string "${ID:-}")" "$(json_string "${VERSION_ID:-}")" \
        "$(json_string "${VERSION_CODENAME:-}")"
    printf '  "packages": [\n%s\n  ]\n}\n' "$packages"
} > "$out"
echo "wrote $out"
