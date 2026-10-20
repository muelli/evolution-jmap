#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This script resolves a .deb's Depends field against the dpkg database of
# its build host.
# It writes the result as JSON for ci/sbom.sh.
# Run this script on the host where the package was built.
# dpkg-shlibdeps derived the Depends field from the libraries installed on
# that host.
# Only that host knows which version of each library the package was linked
# against.
# The release job that assembles the SBOM runs on a different machine.
# That machine does not have most of these libraries installed.
#
# For each Depends clause, the script records the first alternative that
# meets two conditions.
# The alternative must be installed for the native architecture, or marked
# "all".
# The alternative must also satisfy the clause's version constraint.
# A clause with no matching alternative is an error.
# Otherwise the SBOM would name a library version the package was never
# built against.
#
# Output:
#   {"depends": "<the Depends field, verbatim>",
#    "distro": {"id": ..., "version_id": ..., "codename": ...},
#    "packages": [{"name": ..., "version": ..., "arch": ...}, ...]}
#
# This script needs only bash, dpkg, and awk.
# It runs inside the pinned image where the package is built, and that image
# has no jq.
set -euo pipefail

deb="$1"   # path to the built .deb file
out="$2"   # path for the output JSON file

native_arch="$(dpkg --print-architecture)"
depends="$(dpkg-deb -f "$deb" Depends)"

# This function prints argument $1 as a JSON string.
# It escapes the characters that a control field or an os-release value can
# plausibly hold.
# For example, a folded field can bring a literal newline.
# Any other control character would make the JSON invalid.
# ci/sbom.sh's jq step would then reject invalid JSON with an error.
json_string() {
    local s="${1//\\/\\\\}"
    s="${s//\"/\\\"}"
    s="${s//$'\n'/\\n}"
    s="${s//$'\r'/\\r}"
    s="${s//$'\t'/\\t}"
    printf '"%s"' "$s"
}

# This function prints "version<TAB>arch" for the installed instance of
# package $1.
# The instance must be native or architecture-independent.
# It prints nothing if no such instance is installed.
installed() {
    dpkg-query -W -f '${db:Status-Abbrev}\t${Version}\t${Architecture}\n' "$1" \
        2>/dev/null \
        | awk -F'\t' -v native="$native_arch" \
            '$1 ~ /^ii/ && ($3 == native || $3 == "all") { print $2 "\t" $3; exit }' \
        || true
}

# This pattern matches "name[:archqual] [(op version)]".
# It is stored in a variable because [[ =~ ]] cannot take literal
# parentheses inline.
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
