#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Item 74's live-Evolution caveat: the packaged example-module's merged
mail-message-menu item ("My Message Action...", src/m-mail-ui.c) must
actually appear in a real Evolution session, not just link cleanly against
both UI eras. GTK merges a GtkUIManager/EUIManager's XML into real widgets
at shell-view construction, so the item exists in the accessible tree as
soon as the mail shell view is active, with no menu click needed to realize
it: the same assumption ci/gui-smoke.sh's own failure-path full tree dump
already relies on.

Only the main-menu merge is checked here. The sibling mail-folder-popup item
merges into a context menu Evolution builds on demand when a folder is
right-clicked, which this script does not drive; that half of the caveat is
left open for a follow-up.

Run under the same D-Bus session, DISPLAY and XDG environment as the
Evolution instance being asserted about; see ci/gui-smoke-example-module.sh.
"""

import sys
import time

import pyatspi

MENU_ITEM_NAME = "My Message Action..."
POLL_INTERVAL_SECONDS = 2
TIMEOUT_SECONDS = 90


def find_app(name):
    desktop = pyatspi.Registry.getDesktop(0)
    for i in range(desktop.childCount):
        app = desktop.getChildAtIndex(i)
        if app is not None and app.name == name:
            return app
    return None


def find_descendant(node, role=None, name=None, max_depth=20):
    if node is None:
        return None
    try:
        role_matches = role is None or node.getRoleName() == role
        name_matches = name is None or node.name == name
    except Exception:
        return None
    if role_matches and name_matches:
        return node
    if max_depth <= 0:
        return None
    for i in range(node.childCount):
        try:
            child = node.getChildAtIndex(i)
        except Exception:
            continue
        found = find_descendant(child, role, name, max_depth - 1)
        if found is not None:
            return found
    return None


def click(button):
    button.queryAction().doAction(0)


def uncheck(checkbox):
    if checkbox.getState().contains(pyatspi.STATE_CHECKED):
        checkbox.queryAction().doAction(0)


def dismiss_transient_dialogs(evolution):
    """Same one-time dialog ci/gui-smoke-assert.py dismisses; see there for
    why the checkbox is unchecked before clicking OK."""
    auth_dialog = find_descendant(evolution, role="dialog", name="Mail authentication request")
    if auth_dialog is not None:
        checkbox = find_descendant(auth_dialog, role="check box")
        if checkbox is not None:
            uncheck(checkbox)
        ok_button = find_descendant(auth_dialog, role="push button", name="OK")
        if ok_button is not None:
            click(ok_button)
            print("dismissed: Mail authentication request")


def main():
    deadline = time.monotonic() + TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        evolution = find_app("evolution")
        if evolution is not None:
            dismiss_transient_dialogs(evolution)

            item = find_descendant(evolution, role="menu item", name=MENU_ITEM_NAME)
            if item is not None:
                print(f"PASS: menu item {MENU_ITEM_NAME!r} found in the accessible tree")
                return 0

        time.sleep(POLL_INTERVAL_SECONDS)

    print(f"FAIL: menu item {MENU_ITEM_NAME!r} never appeared")
    return 1


if __name__ == "__main__":
    sys.exit(main())
