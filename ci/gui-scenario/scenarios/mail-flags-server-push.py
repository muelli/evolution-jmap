# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: a server-side keyword change reflected in the message list
through live push, with no action in Evolution (ROADMAP item 177(a), the
reverse direction `mail-flags.py` leaves open).

`mail-flags.py` drives the five JMAP keyword changes from the Evolution UI
and asserts the server's `keywords`. This scenario drives the opposite
direction: a plain JMAP `Email/set` call changes `$seen` directly, and the
already-open Evolution window must follow it without a manual refresh. The
folder tree's unread count (`Name (N)`, the same affordance `select_folder`
already waits on) is the one visible, already-reliable signal this harness
has for "the message list changed": it only rises or falls with the read
state of the messages inside, so it proves the client's background sync
picked up the server-side push and re-rendered, not a stale cache.

Not covered here: a client restart after the server-side change (the
harness has no mid-scenario Evolution restart yet, the same gap every
`mail-flags.py` session before this one logged), and the visual star icon
or label colour of the row itself. Both the mail sync engine's push path
(`jmap-mail/tests/push.rs`, `live_server_push.rs`) and the `$seen` write
itself (`jmap-mail-sync/tests/keywords.rs`,
`jmap-mail/tests/live_server_keyword_race.rs`) already have unit coverage;
this scenario is the first to drive a server-initiated keyword change
through the real Evolution UI at all.
"""

import re

from scenario import (
    Jmap,
    ScenarioFailure,
    expect_no_errors,
    folder_cell,
    main,
    say,
    select_folder,
    wait_for,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
SUBJECT = "Welcome to jmap-mockd"
SECOND_SUBJECT = "Second message"
SEEN = "$seen"


def setup():
    write_standalone_mail_account(ACCOUNT)
    say("setup: account sources written")


def unread_count(account, folder):
    """Return the unread count the folder tree cell of `folder` shows, or None
    if the cell is not there yet. The cell name is "Name" with no unread
    message, or "Name (N)" with N unread.
    """
    cell = folder_cell(account, folder)
    if cell is None:
        return None
    name = cell.name
    match = re.fullmatch(rf"{re.escape(folder)}(?: \((\d+)\))?", name)
    if not match:
        raise ScenarioFailure(f"folder cell of {folder!r} has an unexpected name {name!r}")
    return int(match.group(1)) if match.group(1) else 0


def run():
    server = Jmap()
    emails = server.emails()
    message_id = emails[SUBJECT]["id"]
    second_id = emails[SECOND_SUBJECT]["id"]

    select_folder(ACCOUNT, "Inbox")
    wait_for(lambda: unread_count(ACCOUNT, "Inbox") == 2, "Inbox starts with 2 unread messages")
    say("both seeded messages start unread: Inbox (2)")

    server.update("Email", message_id, {f"keywords/{SEEN}": True})
    wait_for(
        lambda: unread_count(ACCOUNT, "Inbox") == 1,
        "Inbox drops to 1 unread after a server-side $seen, no client action",
    )
    say("server sets $seen with no client action: the unread count follows through live push")

    server.update("Email", second_id, {f"keywords/{SEEN}": True})
    wait_for(lambda: unread_count(ACCOUNT, "Inbox") == 0, "Inbox drops to 0 unread")
    say("server marks the second message read too: Inbox shows no unread count")

    server.update("Email", message_id, {f"keywords/{SEEN}": None})
    wait_for(
        lambda: unread_count(ACCOUNT, "Inbox") == 1,
        "Inbox rises to 1 unread after a server-side $seen removal",
    )
    say("server clears $seen with no client action: the unread count rises again through live push")

    server.update("Email", second_id, {f"keywords/{SEEN}": None})
    wait_for(lambda: unread_count(ACCOUNT, "Inbox") == 2, "Inbox rises to 2 unread")
    say("server clears $seen on the second message too: Inbox shows 2 unread again")

    expect_no_errors()


main(setup, run)
