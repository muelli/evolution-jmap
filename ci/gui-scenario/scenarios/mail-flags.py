# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: star/unstar and read/unread on one mail message (ROADMAP item
177(a), the first slice).

Message/Mark As/Important and Unimportant toggle the server's `$flagged`
keyword (Evolution's "star"); Message/Mark As/Read and Unread toggle
`$seen`. The mock seeds both messages with no keywords, so the inbox
starts unstarred and unread.

Not covered here, left for later claims of the same item: the five
built-in labels, a custom label, label removal, junk/not-junk, a
multi-selection, the reverse direction (a server-side keyword change
reflected in the message list), and contacts/calendar categories.
"""

from scenario import (
    Jmap,
    expect_log,
    expect_no_errors,
    main,
    menu_item,
    say,
    select_folder,
    select_message,
    wait_for,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
SUBJECT = "Welcome to jmap-mockd"
KEYWORDS_LOG = 'jmap_mail_sync: setting message keywords .*uid="{}"'


def setup():
    write_standalone_mail_account(ACCOUNT)
    say("setup: account sources written")


def run():
    server = Jmap()
    message_id = server.emails()[SUBJECT]["id"]

    select_folder(ACCOUNT, "Inbox")
    select_message(SUBJECT)
    say(f"starts unstarred and unread: {server.keywords_of(SUBJECT)!r}")

    menu_item("Message/Mark As/Important")
    wait_for(lambda: "$flagged" in server.keywords_of(SUBJECT), f"{SUBJECT!r} flagged")
    say("server: $flagged set")
    expect_log(KEYWORDS_LOG.format(message_id), "the keyword write for Important")

    menu_item("Message/Mark As/Unimportant")
    wait_for(lambda: "$flagged" not in server.keywords_of(SUBJECT), f"{SUBJECT!r} unflagged")
    say("server: $flagged cleared")

    menu_item("Message/Mark As/Unread")
    wait_for(lambda: "$seen" not in server.keywords_of(SUBJECT), f"{SUBJECT!r} unread")
    say("server: $seen cleared")

    menu_item("Message/Mark As/Read")
    wait_for(lambda: "$seen" in server.keywords_of(SUBJECT), f"{SUBJECT!r} read")
    say("server: $seen set")

    expect_no_errors()


main(setup, run)
