# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: a queued move or copy shows at once, offline (ROADMAP item
181(a), 182(b)'s "offline-queue-visible" row).

The manual test of master `f3a62f4f` (F6) found that moving or copying a
message while offline left no visible mark: the row stayed in the source
folder, with no way to tell a change was queued. `jmap-mail` now hides a
queued move's source row (`75b3e80a8`) and shows a queued move or copy in
its destination folder as soon as that folder is next opened (`e83127a2`),
the Camel-layer half already covered by `jmap-mail/tests/transfer.rs`. This
scenario drives the same change through the real Evolution UI: File >
Work Offline, then Message > Move to Folder.../Copy to Folder... on two
messages, checking the message list of each folder before reconnecting.
"""

from scenario import (
    Jmap,
    ScenarioFailure,
    do_action,
    expect_log,
    expect_no_errors,
    find,
    log_count,
    main,
    menu_item,
    message_list,
    pyatspi,
    say,
    screenshot,
    select_folder,
    select_message,
    wait_find,
    wait_for,
    window,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
MOVE_SUBJECT = "Welcome to jmap-mockd"
COPY_SUBJECT = "Second message"
PROJECTS = "Projects"
RECEIPTS = "Receipts"


def setup():
    write_standalone_mail_account(ACCOUNT)
    server = Jmap()
    server.create_mailbox(PROJECTS)
    server.create_mailbox(RECEIPTS)
    say(f"setup: account with the {PROJECTS!r} and {RECEIPTS!r} mailboxes")


def message_shown(subject):
    """Whether `subject` is a showing row of the folder now open."""
    return find(message_list(), "table cell", subject, showing=True) is not None


def gone_from(subject, folder):
    wait_for(lambda: not message_shown(subject), f"{subject!r} gone from {folder!r}")
    say(f"Evolution: {subject!r} left {folder!r}")


def shown_in(subject, folder):
    wait_for(lambda: message_shown(subject), f"{subject!r} shown in {folder!r}")
    say(f"Evolution: {subject!r} shown in {folder!r}")


def transfer_to_folder(subject, folder, *, move):
    """Choose Message > Move/Copy to Folder... on `subject`, to `folder`.

    The dialog's own folder tree shares its accessible name ("Mail Folder
    Tree") with the main window's; every lookup below is rooted at the
    dialog, never at the whole application, to tell the two apart.
    """
    verb = "Move" if move else "Copy"
    select_message(subject)
    menu_item(f"Message/{verb} to Folder…")
    dialog = wait_for(lambda: window(f"{verb} to Folder"), f"the {verb!r} to Folder dialog")
    tree = wait_find("tree table", "Mail Folder Tree", root=dialog)
    cell = wait_for(lambda: find(tree, "table cell", folder), f"folder {folder!r} in the dialog")
    cell.queryComponent().grabFocus()
    screenshot(f"{verb.lower()}-dialog-{folder.lower()}")
    button = wait_find("push button", verb, root=dialog)
    wait_for(lambda: button.getState().contains(pyatspi.STATE_SENSITIVE), f"{verb!r} sensitive")
    do_action(button)
    wait_for(lambda: window(f"{verb} to Folder") is None, f"the {verb!r} to Folder dialog closes")
    say(f"menu Message/{verb} to Folder: {subject!r} to {folder!r}")


def run():
    server = Jmap()
    mailboxes = server.mailboxes()
    inbox_id = mailboxes["Inbox"]["id"]
    projects_id = mailboxes[PROJECTS]["id"]
    receipts_id = mailboxes[RECEIPTS]["id"]

    # Open every folder once online, so Evolution already has a local
    # summary for the two new ones before going offline: the "no downsync
    # for a folder never opened before" gap (item 176) is a separate
    # question from the one this scenario checks.
    select_folder(ACCOUNT, "Inbox")
    select_folder(ACCOUNT, PROJECTS)
    select_folder(ACCOUNT, RECEIPTS)
    select_folder(ACCOUNT, "Inbox")
    screenshot("01-online")

    menu_item("File/Work Offline")
    say("menu: File > Work Offline")

    transfer_to_folder(MOVE_SUBJECT, PROJECTS, move=True)
    gone_from(MOVE_SUBJECT, "Inbox")
    transfer_to_folder(COPY_SUBJECT, RECEIPTS, move=False)
    shown_in(COPY_SUBJECT, "Inbox")
    screenshot("02-inbox-after-queue")

    select_folder(ACCOUNT, PROJECTS)
    shown_in(MOVE_SUBJECT, PROJECTS)
    screenshot("03-projects-queued-move")

    select_folder(ACCOUNT, RECEIPTS)
    shown_in(COPY_SUBJECT, RECEIPTS)
    screenshot("04-receipts-queued-copy")

    if server.mailbox_of(MOVE_SUBJECT) != {inbox_id}:
        raise ScenarioFailure(f"{MOVE_SUBJECT!r} reached the server while offline")
    if server.mailbox_of(COPY_SUBJECT) != {inbox_id}:
        raise ScenarioFailure(f"{COPY_SUBJECT!r} reached the server while offline")
    say("server: neither queued filing was sent while offline")
    if log_count(r"queued filing") < 2:
        raise ScenarioFailure("the log does not show both filings queued")

    menu_item("File/Work Online")
    say("menu: File > Work Online")

    wait_for(
        lambda: server.mailbox_of(MOVE_SUBJECT) == {projects_id},
        f"{MOVE_SUBJECT!r} only in {PROJECTS!r}",
    )
    wait_for(
        lambda: server.mailbox_of(COPY_SUBJECT) == {inbox_id, receipts_id},
        f"{COPY_SUBJECT!r} in both {RECEIPTS!r} and Inbox",
    )
    say("server: the move landed alone, the copy landed alongside the original")
    expect_log(r"replayed filing", "a queued filing replayed on reconnect")
    screenshot("05-replayed-online")

    select_folder(ACCOUNT, "Inbox")
    gone_from(MOVE_SUBJECT, "Inbox")
    shown_in(COPY_SUBJECT, "Inbox")

    expect_no_errors()


main(setup, run)
