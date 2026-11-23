# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: a composer's draft save while offline survives File/Quit and a
restart that starts offline too (ROADMAP item 181, the "draft save, quit
and restart offline" row).

`offline-queue-visible.py` already checks a move, a copy and a delete
queued while offline, replayed on reconnect, in one still-running process.
This scenario checks the harder claim `jmap-mail/src/append.rs` makes for a
saved draft: the row and its bytes must survive not only staying queued,
but the whole process quitting and a new one starting, still offline, with
no network attempt in between. `jmap-functional`'s
`a_draft_appended_while_offline_survives_a_restart_and_uploads_on_reconnect`
proves the same shape directly through libcamel, with no GUI; this drives
it through the real composer and the real shell's offline mode instead.

File > Quit, then a relaunch with `--offline` (`scenario.restart_evolution`,
`online=False`), is the GUI counterpart of that Rust test's second process:
a process that starts already offline, the same as the shell's own "Work
Offline" state, with no reconnect attempt at all until a later File > Work
Online.

With `--live` (item 181's live verification), the account is the seeded
live user's own instead of this scenario's own mock account; the message
is new, not a seeded fixture, so no extra seeding is needed either way.

Save as Draft saves asynchronously and only marks the composer clean once
the save finishes; closing right after it starts re-triggers the "save
changes" prompt for a composer the code still considers dirty (confirmed
with a throwaway probe: Close right after Save as Draft, with no settle,
shows a dialog with "Save Draft"/"Continue Editing"/"Discard Changes").
This settle is well past the mock's own round trip, the same shape
`send-later-offline.py` already uses for the same reason.
"""

import time

from scenario import (
    LIVE_PASSWORD,
    LIVE_USER,
    Jmap,
    ScenarioFailure,
    authenticate,
    expect_log,
    expect_no_errors,
    find,
    labelled_field,
    main,
    menu_item,
    message_list,
    restart_evolution,
    say,
    screenshot,
    select_folder,
    set_text,
    wait_for,
    window,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
RECIPIENT = "bob@example.com"
SUBJECT = "Offline draft restart"
# Well past the async save's own completion, which has no AT-SPI signal to wait on.
SAVE_SETTLE_SECONDS = 3


def setup():
    if LIVE_USER:
        write_standalone_mail_account(ACCOUNT, user=LIVE_USER, drafts_folder=True)
        say("setup: live account source written")
    else:
        write_standalone_mail_account(ACCOUNT, drafts_folder=True)
        say("setup: account source written")


def message_shown(subject):
    """Whether `subject` is a showing row of the folder now open."""
    return find(message_list(), "table cell", subject, showing=True) is not None


def shown_in(subject, folder):
    wait_for(lambda: message_shown(subject), f"{subject!r} shown in {folder!r}")
    say(f"Evolution: {subject!r} shown in {folder!r}")


def run():
    if LIVE_USER:
        authenticate(LIVE_PASSWORD, remember=True, label="authentication-request")
        server = Jmap(user=LIVE_USER, password=LIVE_PASSWORD, rebase=True)
    else:
        server = Jmap()
    drafts_id = server.mailbox_id("drafts")

    # Open Drafts once online, so Evolution already has a local summary for
    # it before going offline: the "no downsync for a folder never opened
    # before" gap (item 176) is a separate question from the one this
    # scenario checks.
    select_folder(ACCOUNT, "Drafts")
    select_folder(ACCOUNT, "Inbox")
    screenshot("01-online")

    menu_item("File/Work Offline")
    say("menu: File > Work Offline")

    menu_item("File/New/Mail Message")
    composer = wait_for(lambda: window("Compose Message"), "the composer window")
    set_text(wait_for(lambda: find(composer, "text", "To:"), "the To field"), RECIPIENT)
    set_text(wait_for(lambda: labelled_field(composer, "Subject:"), "the Subject field"), SUBJECT)
    wait_for(lambda: window(SUBJECT), f"the composer title shows {SUBJECT!r}")
    menu_item("File/Save as Draft", SUBJECT)
    say("composer: File > Save as Draft")
    time.sleep(SAVE_SETTLE_SECONDS)
    menu_item("File/Close", SUBJECT)
    wait_for(lambda: window(SUBJECT) is None, "the composer closes")
    say("composer: closed right after the draft save")
    screenshot("02-draft-saved-offline")

    select_folder(ACCOUNT, "Drafts")
    shown_in(SUBJECT, "Drafts")
    if server.mailbox_of(SUBJECT) is not None:
        raise ScenarioFailure(f"{SUBJECT!r} reached the server while offline")
    say("server: the draft was not sent while offline")

    restart_evolution(online=False)
    say("evolution: restarted, still offline, no reconnect attempt")

    select_folder(ACCOUNT, "Drafts")
    shown_in(SUBJECT, "Drafts")
    screenshot("03-draft-survives-restart")
    if server.mailbox_of(SUBJECT) is not None:
        raise ScenarioFailure(f"{SUBJECT!r} reached the server across the restart")
    say("server: still nothing after the restart, while offline")

    menu_item("File/Work Online")
    say("menu: File > Work Online")

    wait_for(lambda: server.mailbox_of(SUBJECT) == {drafts_id}, f"{SUBJECT!r} lands in Drafts")
    say("server: the draft landed in Drafts on reconnect")
    keywords = server.keywords_of(SUBJECT) or set()
    if "$draft" not in keywords:
        raise ScenarioFailure(f"the uploaded message is not marked a draft: {keywords}")
    expect_log(r"importing message", "the queued append replayed on reconnect")
    screenshot("04-replayed-online")

    expect_no_errors()


main(setup, run)
