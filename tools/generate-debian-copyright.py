#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Two independent generated outputs, selected by argv:
#
# Default: renders docs/packaging/copyright (DEP-5) for this project's own
# source, driven by REUSE.toml's [[annotations]] rather than hand-maintained,
# so a license change there cannot silently leave the shipped copyright file
# wrong.
#
# --third-party-notices: renders docs/packaging/third-party-notices, the
# Track C2 appendix for the ~140 third-party Cargo crates statically linked
# into the built .so's. DEP-5's Files field is a glob over paths in the
# source package, and none of those crates' sources are vendored here or
# shipped in the .deb, so there is no path pattern that honestly describes
# them (a deliberate packaging decision recorded in the project history) —
# hence a separate, non-DEP-5 notices file rather than more Files: stanzas.

import json
import subprocess
import sys
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# The five cdylib crates CMake actually installs as the shipped .so's
# (cmake/Backends.cmake's add_cargo_cdylib() calls) — the dependency closure
# this file's third-party-notices half enumerates. jmap-mail is the Camel
# provider itself, not a *-module wrapper, but it is exactly as shipped as
# the other four.
SHIPPED_CRATES = frozenset(
    [
        "jmap-backend-book-module",
        "jmap-backend-cal-module",
        "jmap-backend-collection-module",
        "jmap-config-module",
        "jmap-mail",
    ]
)

UPSTREAM_CONTACT = "Tobias Mueller <muelli@cryptobitch.de>"
DEFAULT_LICENSE = "GPL-3.0-or-later"
DEFAULT_COPYRIGHT = "2026 Tobias Mueller <muelli@cryptobitch.de>"

LICENSE_TEXT = {
    "GPL-3.0-or-later": """\
 This program is free software: you can redistribute it and/or modify
 it under the terms of the GNU General Public License as published by
 the Free Software Foundation, either version 3 of the License, or
 (at your option) any later version.
 .
 This program is distributed in the hope that it will be useful,
 but WITHOUT ANY WARRANTY; without even the implied warranty of
 MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 GNU General Public License for more details.
 .
 On Debian systems, the complete text of the GNU General Public
 License version 3 can be found in "/usr/share/common-licenses/GPL-3".""",
    "LGPL-2.1-or-later": """\
 This library is free software; you can redistribute it and/or
 modify it under the terms of the GNU Lesser General Public
 License as published by the Free Software Foundation; either
 version 2.1 of the License, or (at your option) any later version.
 .
 This library is distributed in the hope that it will be useful,
 but WITHOUT ANY WARRANTY; without even the implied warranty of
 MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU
 Lesser General Public License for more details.
 .
 On Debian systems, the complete text of the GNU Lesser General
 Public License version 2.1 can be found in
 "/usr/share/common-licenses/LGPL-2.1".""",
}


def load_annotations():
    with (REPO_ROOT / "REUSE.toml").open("rb") as f:
        reuse = tomllib.load(f)
    return reuse["annotations"]


def copyright_lines(value):
    return value if isinstance(value, list) else [value]


def files_field(paths):
    # DEP-5's "*" already matches across "/", so REUSE.toml's doubled-star
    # globs collapse onto it losslessly.
    return " ".join(p.replace("**", "*") for p in paths)


def stanza(files, copyright_value, license_id, comment=None):
    lines = [f"Files: {files}"]
    copy_lines = copyright_lines(copyright_value)
    lines.append(f"Copyright: {copy_lines[0]}")
    lines.extend(f" {c}" for c in copy_lines[1:])
    lines.append(f"License: {license_id}")
    if comment:
        lines.append("Comment:")
        lines.extend(f" {line}" for line in comment)
    return "\n".join(lines)


def render(annotations):
    overrides = [a for a in annotations if a.get("precedence") == "override"]
    aggregates = [a for a in annotations if a.get("precedence") == "aggregate"]

    for a in aggregates:
        if (
            a["SPDX-License-Identifier"] != DEFAULT_LICENSE
            or copyright_lines(a["SPDX-FileCopyrightText"]) != [DEFAULT_COPYRIGHT]
        ):
            raise SystemExit(
                "REUSE.toml aggregate annotation no longer matches this "
                "script's DEFAULT_LICENSE/DEFAULT_COPYRIGHT — update both "
                "together, since the default Files: * stanza below is only "
                "correct while they agree: "
                f"{a['path']!r}"
            )

    parts = [
        "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/",
        "Upstream-Name: evolution-jmap",
        f"Upstream-Contact: {UPSTREAM_CONTACT}",
        "Source: https://github.com/muelli/evolution-jmap",
        "",
        stanza(
            "*",
            DEFAULT_COPYRIGHT,
            DEFAULT_LICENSE,
            comment=[
                "Covers this project's own source not named by a more specific",
                "Files stanza below (checked by `reuse lint` against",
                "REUSE.toml and each file's own SPDX header). Generated by",
                "tools/generate-debian-copyright.py from REUSE.toml — do not",
                "hand-edit the Files stanzas; run the script instead.",
                ".",
                "Does not enumerate the license of the third-party Rust",
                "crates statically linked into the shipped modules: DEP-5's",
                "Files field is a path glob over the source package, and",
                "none of those crates' sources are vendored here or shipped",
                "in this .deb, so no path pattern would honestly describe",
                "them. See docs/packaging/third-party-notices instead",
                "(Track C2, also generated by",
                "tools/generate-debian-copyright.py).",
            ],
        ),
        "",
    ]

    for a in overrides:
        parts.append(
            stanza(
                files_field(a["path"]),
                a["SPDX-FileCopyrightText"],
                a["SPDX-License-Identifier"],
            )
        )
        parts.append("")

    seen_licenses = {DEFAULT_LICENSE} | {a["SPDX-License-Identifier"] for a in overrides}
    for license_id in sorted(seen_licenses):
        text = LICENSE_TEXT.get(license_id)
        if text is None:
            raise SystemExit(f"no License paragraph text on file for {license_id}")
        parts.append(f"License: {license_id}\n{text}")
        parts.append("")

    return "\n".join(parts).rstrip("\n") + "\n"


def cargo_metadata():
    result = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked"],
        cwd=REPO_ROOT / "rust",
        capture_output=True,
        text=True,
        check=True,
    )
    return json.loads(result.stdout)


def third_party_dependency_closure(metadata):
    # Walks only "normal" (non-dev, non-build) dependency edges: build-deps
    # (bindgen, pkg-config, ...) compile into separate host-side build
    # scripts, never into the shipped cdylib, and dev-deps never compile
    # outside `cargo test`. Either would list a crate that is not actually
    # in the .so.
    packages = {p["id"]: p for p in metadata["packages"]}
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}

    roots = [
        pid
        for pid, p in packages.items()
        if p["name"] in SHIPPED_CRATES and p["source"] is None
    ]
    missing = SHIPPED_CRATES - {packages[pid]["name"] for pid in roots}
    if missing:
        raise SystemExit(
            f"cargo metadata did not resolve these SHIPPED_CRATES as "
            f"workspace members: {sorted(missing)} — crate renamed or "
            f"removed? Update SHIPPED_CRATES to match."
        )

    seen = set()
    queue = list(roots)
    closure = set()
    while queue:
        pid = queue.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]["deps"]:
            if not any(kind["kind"] is None for kind in dep["dep_kinds"]):
                continue
            closure.add(dep["pkg"])
            if dep["pkg"] not in seen:
                queue.append(dep["pkg"])

    third_party = [
        packages[pid]
        for pid in closure
        if packages[pid]["source"] and packages[pid]["source"].startswith("registry+")
    ]
    third_party.sort(key=lambda p: (p["name"], p["version"]))

    unlicensed = [p["name"] for p in third_party if not p["license"]]
    if unlicensed:
        raise SystemExit(
            "cargo metadata reports no SPDX license expression for: "
            f"{unlicensed} — resolve by hand before regenerating "
            "(license_file-only crates need a human to read the file)."
        )

    return third_party


def render_third_party_notices():
    third_party = third_party_dependency_closure(cargo_metadata())
    lines = [
        "This is a supplementary notice, not a DEP-5 debian/copyright file: it",
        "lists the third-party Rust crates statically linked into",
        "evolution-jmap's shipped .so modules. None of their sources are",
        "vendored or shipped in this .deb, so DEP-5's Files: field (a path",
        "glob over the source package) has no honest pattern for them; see",
        "docs/packaging/copyright's own comment for the rationale.",
        "",
        "Generated by tools/generate-debian-copyright.py from `cargo",
        "metadata` — do not hand-edit; regenerate instead.",
        "",
    ]
    for p in third_party:
        lines.append(f"{p['name']} {p['version']} -- {p['license']} -- {p['repository']}")
    return "\n".join(lines) + "\n"


def main():
    if "--third-party-notices" in sys.argv[1:]:
        sys.stdout.write(render_third_party_notices())
    else:
        sys.stdout.write(render(load_annotations()))


if __name__ == "__main__":
    main()
