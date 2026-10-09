#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Item 66's live-Evolution caveat: a brand-new JMAP collection account's
first populate must authenticate ONCE, not twice — `on_account_changed`
must not answer a `"changed"` emission its own populate caused.

The oracle is jmap-mock's own request log rather than another AT-SPI read:
`jmap_mock::dispatch::handle_api` prints every POST /jmap body to stdout
(`GUI_SMOKE_MOCK_LOG`, redirected there by
ci/gui-smoke-collection-populate-once.sh), and this profile's one Evolution
instance is the only client that ever opens this account's mail store, so
`Mailbox/get` traffic maps directly onto how many times it connected.
Its baseline is not 1, though: a control run of ci/gui-smoke.sh's own
standalone mail account (no collection backend, no `on_account_changed` in
the picture at all) empirically shows `Mailbox/get` firing twice on every
healthy single connect too, confirmed 2026-10-09 rather than assumed — once
for the store's own `connect_sync`, once more for Evolution's own
folder-tree refresh right after. A bugged second populate doubles that
again, to 4, not to 2 — which is also why the fixture this script drives
(ci/gui-smoke-collection-populate-once.sh) turns contacts and calendars OFF
on the collection account: both get their own independently-opening EDS
client (autocompletion, the reminder watcher) whose connect has nothing to
do with this account's own populate, and counting their request traffic
the same way was confirmed noisy before this script settled on mail alone.

Run under the same D-Bus session, DISPLAY and XDG environment as the
Evolution instance being asserted about; see
ci/gui-smoke-collection-populate-once.sh.
"""

import os
import re
import sys
import time

import pyatspi

ACCOUNT_NAME = "JMAP mock mail-only account"
POLL_INTERVAL_SECONDS = 2
TIMEOUT_SECONDS = 90
SETTLE_SECONDS = 15
HEALTHY_MAILBOX_GET_COUNT = 2
INBOX_PATTERN = re.compile(r"^Inbox \((\d+)\)$")


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
    """Same one-time dialog ci/gui-smoke-assert.py dismisses. Dismissed
    every poll, not just once: a bugged second populate would show it
    again, and leaving it up would stall that second login rather than let
    it reach the mock where this script can see it."""
    auth_dialog = find_descendant(evolution, role="dialog", name="Mail authentication request")
    if auth_dialog is not None:
        checkbox = find_descendant(auth_dialog, role="check box")
        if checkbox is not None:
            uncheck(checkbox)
        ok_button = find_descendant(auth_dialog, role="push button", name="OK")
        if ok_button is not None:
            click(ok_button)
            print("dismissed: Mail authentication request")


def account_inbox_count(evolution):
    tree = find_descendant(evolution, role="tree table", name="Mail Folder Tree")
    if tree is None:
        return None

    cells = list(all_descendants(tree, role="table cell"))
    names = [cell.name for cell in cells if cell.name]
    if ACCOUNT_NAME not in names:
        return None

    account_index = names.index(ACCOUNT_NAME)
    for name in names[account_index + 1 :]:
        match = INBOX_PATTERN.match(name)
        if match is not None:
            return int(match.group(1))
        if name == ACCOUNT_NAME:
            break
    return None


def mailbox_get_count(mock_log_path):
    """Count real `Mailbox/get` invocations, not the method name echoed back
    in every response too: `jmap_mock::dispatch::handle_api` logs each
    request body (one compact JSON line) straight after its own
    `--> POST /jmap` marker, so only the line immediately following that
    marker is a request to count, never the pretty-printed response below
    it."""
    try:
        with open(mock_log_path, encoding="utf-8", errors="replace") as handle:
            lines = handle.readlines()
    except OSError as error:
        print(f"could not read mock log {mock_log_path!r}: {error}")
        return None
    count = 0
    for i, line in enumerate(lines):
        if line.strip() == "--> POST /jmap" and i + 1 < len(lines):
            if '"Mailbox/get"' in lines[i + 1]:
                count += 1
    return count


def main():
    mock_log_path = os.environ.get("GUI_SMOKE_MOCK_LOG")
    if not mock_log_path:
        print("FAIL: GUI_SMOKE_MOCK_LOG not set")
        return 2

    deadline = time.monotonic() + TIMEOUT_SECONDS
    account_seen = False
    inbox_seen_at = None
    while time.monotonic() < deadline:
        evolution = find_app("evolution")
        if evolution is not None:
            dismiss_transient_dialogs(evolution)

            tree = find_descendant(evolution, role="tree table", name="Mail Folder Tree")
            if tree is not None and not account_seen:
                names = [cell.name for cell in all_descendants(tree, role="table cell") if cell.name]
                account_seen = ACCOUNT_NAME in names

            count = account_inbox_count(evolution)
            if count is not None and count > 0 and inbox_seen_at is None:
                inbox_seen_at = time.monotonic()
                print(f"account {ACCOUNT_NAME!r} inbox has {count} message(s), settling {SETTLE_SECONDS}s")

        if inbox_seen_at is not None and time.monotonic() - inbox_seen_at >= SETTLE_SECONDS:
            count = mailbox_get_count(mock_log_path)
            if count is None:
                return 2
            if count == HEALTHY_MAILBOX_GET_COUNT:
                print(f"PASS: mock saw {count} Mailbox/get calls (one healthy connect) after settling")
                return 0
            print(
                f"FAIL: mock saw {count} Mailbox/get calls after settling, "
                f"expected {HEALTHY_MAILBOX_GET_COUNT} (one healthy connect)"
            )
            return 1

        time.sleep(POLL_INTERVAL_SECONDS)

    if not account_seen:
        print(f"FAIL: account {ACCOUNT_NAME!r} never appeared in the mail folder tree")
    else:
        print(f"FAIL: account {ACCOUNT_NAME!r} appeared but its inbox never became non-empty")
    return 1


if __name__ == "__main__":
    sys.exit(main())
