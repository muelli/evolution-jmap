# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: selecting and refreshing the synthetic "Shared" folders
(ROADMAP item 179(b), 182(b)'s "shared-synthetic-refresh" row).

`jmap-mockd --share` grants the default account a mailbox from a second,
non-personal account. `merge_shared` adds the mailbox under a synthetic
"Shared" root and an owner heading below it, neither a real mailbox a
server has ever heard of. Before the 179(b) fix, selecting or refreshing
one of these sent its made-up id to the server and got back
`invalidArguments: invalid JMAP ID`. `refresh_info_sync` now treats a
synthetic folder id as a no-op (`jmap_mail_sync::is_synthetic_folder_id`).

This scenario selects, then explicitly refreshes, both the "Shared" root
and the "Shared/<owner>" heading, and checks that neither logs an error.
"""

from scenario import (
    do_action,
    expect_no_errors,
    folder_cell,
    main,
    menu_item,
    say,
    screenshot,
    select_folder,
    wait_for,
    write_collection_mail_account,
)

ACCOUNT = "JMAP scenario mail"
MOCK_ARGS = ("--share",)
SHARED_ROOT = "Shared"
OWNER_HEADING = "owner@example.test"


def setup():
    write_collection_mail_account(ACCOUNT)
    say("setup: a mail account with a mailbox shared from a second account")


def select_and_refresh(folder, step):
    select_folder(ACCOUNT, folder)
    say(f"selected {folder!r}")
    menu_item("Folder/Refresh")
    say(f"refreshed {folder!r}")
    screenshot(step)
    expect_no_errors()


def expand_shared_root():
    """Expand the "Shared" row so its owner heading has a tree cell.

    The cell's own "expand or contract" AT-SPI action toggles it directly,
    unlike a click or a key press: a `GtkTreeView` row's disclosure triangle
    is not a separate accessible object to click, and an arrow key only
    expands a row that already has real keyboard focus.
    """
    cell = wait_for(
        lambda: folder_cell(ACCOUNT, SHARED_ROOT), f"{SHARED_ROOT!r} in the folder tree"
    )
    do_action(cell, "expand or contract")
    wait_for(
        lambda: folder_cell(ACCOUNT, OWNER_HEADING), f"{OWNER_HEADING!r} in the folder tree"
    )
    say(f"expanded {SHARED_ROOT!r}")


def run():
    select_folder(ACCOUNT, "Inbox")
    say("online: the folder tree shows the account")

    select_and_refresh(SHARED_ROOT, "01-shared-root")
    expand_shared_root()
    select_and_refresh(OWNER_HEADING, "02-owner-heading")

    expect_no_errors()


main(setup, run, mock_args=MOCK_ARGS)
