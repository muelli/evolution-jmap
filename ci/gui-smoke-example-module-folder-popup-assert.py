#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Item 74's other live-Evolution caveat: the packaged example-module's merged
mail-folder-popup item ("My Maildir Folder Action...", src/m-mail-ui.c) must
actually appear in the folder tree's real context menu.

Unlike the mail-message-menu item (ci/gui-smoke-example-module-assert.py),
Evolution builds this context menu fresh via e_ui_manager_create_item every
time a folder is right-clicked, well after the shell view and its extensions
are constructed, so a static tree walk proves nothing here: the item is
absent from the tree until the popup is actually triggered. This script
drives a real right-click through AT-SPI's synthetic pointer
(pyatspi.Registry.generateMouseEvent at the account row's own Component
extents), the same mechanism a user's mouse would generate.

Getting a real (x, y) for that click and having the click actually reach
Evolution both depend on machinery ci/gui-smoke.sh and
ci/gui-smoke-example-module.sh never needed, because they only ever drive
AT-SPI's Action interface (a direct, in-process ATK call, no X input
involved): a window manager (ci/gui-smoke-example-module-folder-popup.sh
now starts openbox; GDK cannot resolve a toplevel's absolute screen position
without one, and Component.getExtents comes back as GLib's "unknown"
sentinel, G_MININT32, not a real coordinate, until one is running), and
pushing DISPLAY into the D-Bus activation environment (same script,
dbus-update-activation-environment; at-spi2-registryd, which actually owns
the XTestFakeButtonEvent call this makes, is D-Bus-activated with its own
environment, not the caller's, so without this the synthetic click silently
reaches no display at all).

The item is expected to appear but stay insensitive: its own sensitivity
gate (REQUIRE_SERVICE_PROTOCOL "maildir" in src/m-mail-ui.c) never matches
this backend's JMAP provider, and m_utils_enable_actions only ever toggles
sensitivity, never visibility. So this only asserts presence in the
accessible tree, not enabled state.

Run under the same D-Bus session, DISPLAY and XDG environment as the
Evolution instance being asserted about; see
ci/gui-smoke-example-module-folder-popup.sh.
"""

import sys
import time

import pyatspi

ACCOUNT_NAME = "JMAP mock mail"
MENU_ITEM_NAME = "My Maildir Folder Action..."
POLL_INTERVAL_SECONDS = 2
TIMEOUT_SECONDS = 90
POPUP_WAIT_SECONDS = 2


def find_app(names):
    """`names` is an iterable of acceptable AT-SPI application names: the
    name Evolution registers under differs by environment, "evolution" in
    the minimal rootless-podman Ubuntu container the EDS 3.52 leg uses, but
    "org.gnome.Evolution" (its GApplication id) in the full-desktop Fedora
    3.60.2 container, which has desktop-file-utils/shared-mime-info wired up
    enough for GApplication's own D-Bus activation naming to take over."""
    desktop = pyatspi.Registry.getDesktop(0)
    for i in range(desktop.childCount):
        app = desktop.getChildAtIndex(i)
        if app is not None and app.name in names:
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


def find_button(node, name):
    """The push-button role name itself differs by AT-SPI stack: "push
    button" in the minimal rootless-podman Ubuntu container the EDS 3.52 leg
    uses, plain "button" in the full-desktop Fedora 3.60.2 container's newer
    at-spi2-core. Role is not filtered at all here (name alone is specific
    enough for every caller) so neither spelling has to be guessed."""
    return find_descendant(node, role=None, name=name)


def all_descendants(node, role=None, max_depth=20):
    if node is None or max_depth < 0:
        return
    try:
        if role is None or node.getRoleName() == role:
            yield node
    except Exception:
        return
    for i in range(node.childCount):
        try:
            child = node.getChildAtIndex(i)
        except Exception:
            continue
        yield from all_descendants(child, role, max_depth - 1)


def click(button):
    button.queryAction().doAction(0)


def uncheck(checkbox):
    if checkbox.getState().contains(pyatspi.STATE_CHECKED):
        checkbox.queryAction().doAction(0)


def dismiss_transient_dialogs(evolution):
    """Same one-time dialog ci/gui-smoke-assert.py dismisses; see there for
    why the checkbox is unchecked before clicking OK.

    A second, unrelated one-time dialog ("Do you want to make Evolution your
    default email client?") showed up only in the full-desktop Fedora 3.60.2
    container this script's EDS 3.60.2 leg runs in, never in the minimal
    rootless-podman Ubuntu 24.04 container the EDS 3.52 leg uses: a fresh
    XDG_DATA_HOME has no mimeapps.list recording Evolution as already
    declined, and only the Fedora container has desktop-file-utils/
    shared-mime-info installed as part of the full `evolution` package to
    make that check fire at all. "Do not change settings" leaves the
    throwaway container's state untouched, same spirit as unchecking the
    auth dialog's "remember password" box above."""
    auth_dialog = find_descendant(evolution, role="dialog", name="Mail authentication request")
    if auth_dialog is not None:
        checkbox = find_descendant(auth_dialog, role="check box")
        if checkbox is not None:
            uncheck(checkbox)
        ok_button = find_button(auth_dialog, "OK")
        if ok_button is not None:
            click(ok_button)
            print("dismissed: Mail authentication request")

    default_client_button = find_button(evolution, "Do not change settings")
    if default_client_button is not None:
        click(default_client_button)
        print("dismissed: Do you want to make Evolution your default email client?")


def find_account_cell(evolution):
    """The mock account's own row in the folder tree. Right-clicking the
    account's root node reaches the same mail-folder-popup merge as
    right-clicking one of its subfolders (get_store_from_folder_tree and
    mail_update_state in src/m-mail-ui.c both handle the "account root
    selected" case explicitly), and the account row is present as soon as
    its .source loads, with no need to wait for the account to finish
    connecting or for its row to be expanded to reveal Inbox."""
    tree = find_descendant(evolution, role="tree table", name="Mail Folder Tree")
    if tree is None:
        return None

    for cell in all_descendants(tree, role="table cell"):
        if cell.name == ACCOUNT_NAME:
            return cell
    return None


def right_click(cell):
    # grabFocus scrolls the row into view and makes the tree realize it, which
    # a freshly-added account row needs before its Component extents resolve
    # to a real, on-screen (x, y) rather than a stale or default one.
    cell.queryComponent().grabFocus()
    time.sleep(0.3)
    extents = cell.queryComponent().getExtents(pyatspi.XY_SCREEN)
    x = extents.x + extents.width // 2
    y = extents.y + extents.height // 2
    pyatspi.Registry.generateMouseEvent(x, y, pyatspi.MOUSE_B3C)


def find_popup_item(evolution):
    for menu in all_descendants(evolution, role="menu"):
        item = find_descendant(menu, role="menu item", name=MENU_ITEM_NAME)
        if item is not None:
            return item
    return None


def main():
    deadline = time.monotonic() + TIMEOUT_SECONDS
    right_clicked = False
    next_attempt_deadline = 0.0
    while time.monotonic() < deadline:
        evolution = find_app(("evolution", "org.gnome.Evolution"))
        if evolution is not None:
            dismiss_transient_dialogs(evolution)

            if time.monotonic() >= next_attempt_deadline:
                cell = find_account_cell(evolution)
                if cell is not None:
                    right_click(cell)
                    right_clicked = True
                    next_attempt_deadline = time.monotonic() + POPUP_WAIT_SECONDS + POLL_INTERVAL_SECONDS
                    time.sleep(POPUP_WAIT_SECONDS)

            if right_clicked:
                item = find_popup_item(evolution)
                if item is not None:
                    print(f"PASS: menu item {MENU_ITEM_NAME!r} found in the folder context menu")
                    return 0

        time.sleep(POLL_INTERVAL_SECONDS)

    if not right_clicked:
        print(f"FAIL: never found the {ACCOUNT_NAME!r} cell to right-click")
    else:
        print(f"FAIL: menu item {MENU_ITEM_NAME!r} never appeared after the right-click")
    return 1


if __name__ == "__main__":
    sys.exit(main())
