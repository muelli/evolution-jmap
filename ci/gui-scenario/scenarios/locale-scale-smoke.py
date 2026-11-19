# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: the --locale and --scale harness itself (item 174), not a
layout or wording check of any page.

Item 174's open row needs a scenario at 800x600, German and 200% text; no
harness ran a scenario under another locale or text scale at all before
this one. This scenario is the proof that ci/gui-scenario/run.sh --locale
and --scale reach the session Evolution starts in: run it plain for the C
locale and 100% default, or with `--locale de_DE.UTF-8 --scale 200` for
the combination item 174 needs.

It does not check a folder or message name: under a real locale, Evolution
translates its own widget names too (the folder tree becomes
"E-Mail-Ordnerbaum", not "Mail Folder Tree"), so a check keyed to an
English name belongs to a page-specific scenario, not this one.
"""

import os
import subprocess

from scenario import (
    ScenarioFailure,
    evolution,
    expect_no_errors,
    find,
    main,
    say,
    screenshot,
    wait_for,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"


def setup():
    write_standalone_mail_account(ACCOUNT)
    say("setup: account sources written")


def _text_scaling_factor():
    result = subprocess.run(
        ["gsettings", "get", "org.gnome.desktop.interface", "text-scaling-factor"],
        capture_output=True,
        text=True,
        check=True,
    )
    return float(result.stdout.strip())


def run():
    locale = os.environ.get("GUI_SCENARIO_LOCALE", "")
    scale = os.environ.get("GUI_SCENARIO_SCALE", "")
    lang = os.environ.get("LANG", "")
    say(f"run.sh --locale {locale or '(unset)'} --scale {scale or '(unset)'}")

    expected_lang = locale or "C"
    if lang != expected_lang:
        raise ScenarioFailure(f"LANG is {lang!r}, expected {expected_lang!r}")
    say(f"LANG reached the session: {lang!r}")

    factor = _text_scaling_factor()
    expected_factor = (float(scale) / 100) if scale else 1.0
    if factor != expected_factor:
        raise ScenarioFailure(f"text-scaling-factor is {factor}, expected {expected_factor}")
    say(f"text-scaling-factor reached the session: {factor}")

    # Evolution itself must still start and show a window under the
    # requested locale and scale: a smoke check, not a layout or wording one.
    wait_for(lambda: find(evolution(), "frame") is not None, "a top-level Evolution window")
    screenshot("locale-scale-smoke")
    if evolution() is None:
        raise ScenarioFailure("Evolution is not running")
    expect_no_errors()


main(setup, run)
