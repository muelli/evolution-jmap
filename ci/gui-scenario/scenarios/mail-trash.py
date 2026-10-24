# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: the trash of a JMAP mail account.

1. Delete in the inbox moves the message to the server's trash mailbox.
2. Delete and Expunge in the trash destroy the message on the server.
3. Empty Trash destroys every message in the server's trash.

The mock seeds two messages in the inbox and no trash. The setup step adds a
mailbox with the "trash" role.
"""

from scenario import (
    Jmap,
    ScenarioFailure,
    confirm,
    expect_log,
    expect_no_errors,
    main,
    menu_item,
    say,
    screenshot,
    select_folder,
    select_message,
    wait_for,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
FIRST = "Welcome to jmap-mockd"
SECOND = "Second message"
TRASHED = 'jmap_mail_sync: trashing message .*uid="{}"'
EXPUNGED = 'jmap_mail_sync: expunging message .*uid="{}"'


def setup():
    write_standalone_mail_account(ACCOUNT)
    Jmap().create_mailbox("Trash", "trash")
    say("setup: account sources written, trash mailbox created")


def run():
    server = Jmap()
    inbox = server.mailbox_id("inbox")
    trash = server.mailbox_id("trash")
    first_id = server.emails()[FIRST]["id"]
    second_id = server.emails()[SECOND]["id"]

    select_folder(ACCOUNT, "Inbox")
    select_message(FIRST)
    screenshot("inbox-before-delete")

    menu_item("Edit/Delete Message")
    wait_for(lambda: server.mailbox_of(FIRST) == {trash}, f"{FIRST!r} only in the server's trash")
    say(f"server: {FIRST!r} moved from the inbox to the trash")
    expect_log(TRASHED.format(first_id), f"the move of {first_id}")
    screenshot("inbox-after-delete")

    select_folder(ACCOUNT, "Trash")
    select_message(FIRST)
    screenshot("trash-before-expunge")
    menu_item("Edit/Delete Message")
    menu_item("Folder/Expunge")
    confirm("Expunge", "expunge-confirmation")
    wait_for(lambda: server.mailbox_of(FIRST) is None, f"{FIRST!r} destroyed on the server")
    say(f"server: {FIRST!r} destroyed")
    expect_log(EXPUNGED.format(first_id), f"the destroy of {first_id}")
    screenshot("trash-after-expunge")

    select_folder(ACCOUNT, "Inbox")
    select_message(SECOND)
    menu_item("Edit/Delete Message")
    wait_for(lambda: server.mailbox_of(SECOND) == {trash}, f"{SECOND!r} only in the server's trash")
    say(f"server: {SECOND!r} moved to the trash, not marked for removal")
    expect_log(TRASHED.format(second_id), f"the move of {second_id}")

    menu_item("File/Empty Trash")
    confirm("Empty Trash", "empty-trash-confirmation")
    wait_for(lambda: server.mailbox_of(SECOND) is None, f"{SECOND!r} destroyed on the server")
    remaining = [s for s, e in server.emails().items() if trash in e["mailboxIds"]]
    if remaining:
        raise ScenarioFailure(f"the server's trash still holds {remaining}")
    if inbox in {m for e in server.emails().values() for m in e["mailboxIds"]}:
        raise ScenarioFailure("the server's inbox is not empty")
    say("server: the trash is empty")
    expect_log(EXPUNGED.format(second_id), f"the destroy of {second_id}")
    expect_no_errors()
    select_folder(ACCOUNT, "Trash")
    screenshot("trash-after-empty-trash")


main(setup, run)
