#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""M9 Tier 2's assertion: drive a running Evolution through AT-SPI and check
that the JMAP account this run configured appears in the mail folder tree
with a non-empty inbox.

Plain `pyatspi` rather than `dogtail`: this is one scripted assertion with no
predicate reuse across tests, and dogtail's tree/predicate layer is a
convenience over exactly the calls used here — an extra dependency this
script does not need to earn its keep.

Run under the same D-Bus session, `DISPLAY` and XDG environment as the
Evolution instance being asserted about; see `ci/gui-smoke.sh`.
"""

import re
import sys
import time

import pyatspi

ACCOUNT_NAME = "JMAP mock mail"
POLL_INTERVAL_SECONDS = 2
TIMEOUT_SECONDS = 90
INBOX_PATTERN = re.compile(r"^Inbox \((\d+)\)$")


def find_app(name):
    desktop = pyatspi.Registry.getDesktop(0)
    for i in range(desktop.childCount):
        app = desktop.getChildAtIndex(i)
        if app is not None and app.name == name:
            return app
    return None


def find_descendant(node, role=None, name=None, name_prefix=None, max_depth=20):
    if node is None:
        return None
    try:
        role_matches = role is None or node.getRoleName() == role
        name_matches = name is None or node.name == name
        prefix_matches = name_prefix is None or (node.name or "").startswith(name_prefix)
    except Exception:
        return None
    if role_matches and name_matches and prefix_matches:
        return node
    if max_depth <= 0:
        return None
    for i in range(node.childCount):
        try:
            child = node.getChildAtIndex(i)
        except Exception:
            continue
        found = find_descendant(child, role, name, name_prefix, max_depth - 1)
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
    """Best-effort: click through the one-time dialog a fresh, keyring-less
    profile shows on its first connection. Not part of what this test
    asserts — see docs/gui-smoke-test.md.

    The "remember password" checkbox is unchecked first, deliberately: EDS
    treats a failure to *store* the credential (there is no keyring service
    in this scratch profile) as a failure to *authenticate*, retried a bounded
    number of times before the account gives up with a hard "Failed to
    connect" — leaving nothing to store sidesteps that path entirely rather
    than fighting the retry-and-give-up timing.
    """
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
    # The account's children are the cells listed after it, up to the next
    # account-level node (Evolution has no other reliable way through AT-SPI
    # to say "this cell's parent is that one" in a tree table). The account's
    # own Inbox is always its first child in the JMAP collection's fan-out
    # order, which this script does not control — it reads it back.
    for name in names[account_index + 1 :]:
        match = INBOX_PATTERN.match(name)
        if match is not None:
            return int(match.group(1))
        if name == ACCOUNT_NAME:
            break
    return None


def main():
    deadline = time.monotonic() + TIMEOUT_SECONDS
    account_seen = False
    while time.monotonic() < deadline:
        evolution = find_app("evolution")
        if evolution is not None:
            dismiss_transient_dialogs(evolution)

            tree = find_descendant(evolution, role="tree table", name="Mail Folder Tree")
            if tree is not None and not account_seen:
                names = [cell.name for cell in all_descendants(tree, role="table cell") if cell.name]
                account_seen = ACCOUNT_NAME in names

            count = account_inbox_count(evolution)
            if count is not None:
                if count > 0:
                    print(f"PASS: account {ACCOUNT_NAME!r} appeared, inbox has {count} message(s)")
                    return 0
                print(f"waiting: account appeared, inbox still empty (count={count})")

        time.sleep(POLL_INTERVAL_SECONDS)

    if not account_seen:
        print(f"FAIL: account {ACCOUNT_NAME!r} never appeared in the mail folder tree")
    else:
        print(f"FAIL: account {ACCOUNT_NAME!r} appeared but its inbox never became non-empty")
    return 1


if __name__ == "__main__":
    sys.exit(main())
