# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: a shared mail account's mailboxes show in the folder tree
(ROADMAP items 163 and 179(a), 182(b)'s "shared-folders-tree" row).

jmap-mock's `--share` mode seeds a second, non-personal account
("owner@example.test") that shares two mailboxes with the login
account: "Shared Read Only" and "Shared Read Write", each with its own
seeded messages, plus a "Private" mailbox kept unshared.
`FolderTree::merge_shared` (jmap-mail-sync) merges a shared account's
mailboxes under one always-present "Shared" root with one child
heading per owner, the shape evolution-ews and IMAP shared namespaces
use (finding F3 of `notes/MANUAL-TEST-2026-11-10.md`: the merged data
was already correct, but a real Evolution showed no children under
the owner heading). This scenario drives that merge through a real
folder tree instead of the unit tests' own `FolderTree` fixtures.

Not covered here, both live-only: a nested folder under a shared
owner (the roadmap item's own "Q4 under Team Shared" example; the mock
seeds no nested mailbox) and a share appearing or vanishing while
Evolution runs (`share-new`/`unshare`, item 182(a)'s `--live` mode,
blocked headless on gnome-keyring's unlock prompt as of 2026-11-14).

Also not covered: that the unshared "Private" mailbox never reaches
the client at all. The mock's share filtering
(`jmap-mock::mail::visible_mailbox`) keys off the *authenticated
caller's* principal, but a plain `write_standalone_mail_account`
connection sends no `Authorization` header, so the mock treats it as
the account's own owner for any account it names (`caller.is_none_or`
in `jmap-mock::mail::mailbox_get`) and returns every mailbox,
`Private` included. Exercising the real exclusion needs a caller the
mock can tell apart from the owner (bearer auth bound to a principal,
`--bearer-as`), which needs its own account-setup work; left for a
follow-up claim or the live target.
"""

import pyatspi

from scenario import (
    ScenarioFailure,
    do_action,
    expect_no_errors,
    folder_cell,
    main,
    say,
    screenshot,
    select_folder,
    select_message,
    wait_for,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
OWNER = "owner@example.test"


def setup():
    write_standalone_mail_account(ACCOUNT)
    say("setup: account sources written")


def expand(folder):
    """Select `folder` in ACCOUNT's tree and expand it to reveal its
    children, if it has any and is not already expanded.

    A synthetic Right arrow key press (the usual way to expand a GTK tree
    row) is not reliably delivered to the folder tree in this AT-SPI/
    Xvfb setup, confirmed empirically: the cell's own "expand or
    contract" action works every time, so this uses that action instead.
    """
    select_folder(ACCOUNT, folder)
    cell = wait_for(lambda: folder_cell(ACCOUNT, folder), f"folder {folder!r} in the tree")
    if not cell.getState().contains(pyatspi.STATE_EXPANDED):
        do_action(cell, "expand or contract")


def run():
    select_folder(ACCOUNT, "Inbox")
    screenshot("01-own-inbox")
    say(f"own account: {ACCOUNT!r} shows its own Inbox")

    expand("Shared")
    say('expanded the synthetic "Shared" root')
    expand(OWNER)
    say(f"expanded the owner heading {OWNER!r}")
    screenshot("02-shared-expanded")

    for folder, subjects in (
        ("Shared Read Only", ("Read-only message 1", "Read-only message 2")),
        ("Shared Read Write", ("Read-write message 1", "Read-write message 2")),
    ):
        select_folder(ACCOUNT, folder)
        for subject in subjects:
            select_message(subject)
        screenshot(f"03-{folder.lower().replace(' ', '-')}")
        say(f"shared folder {folder!r} shows its seeded messages")

    expect_no_errors()


main(setup, run, mock_args=("--share",))
