# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: File > Work Offline (ROADMAP item 176(a), the "File > Work
Offline" row of the offline matrix).

Evolution's "work-offline"/"work-online" File menu actions call
e_shell_set_online() directly (e-shell-window-actions.c:
action_work_offline_cb/action_work_online_cb), independent of
GNetworkMonitor's "network-available" property, which --force-online (used
by every scenario's launch) only locks true. So this toggle exercises the
real user-facing offline mode even though the mock stays reachable the
whole time; this scenario cannot simulate an actual network outage (no way
to fail the mock's TCP port without also killing the display session).

Checks: the folder tree and message list of an already-synced account stay
usable after File > Work Offline (176(a)'s "folder tree"/"message list"
rows, now through the real shell toggle instead of a restarted process). A
flag change made while offline is queued, not sent, and flushes once File >
Work Online runs (176(a)'s "flag changes" row; the write cannot be
reasonably told apart from "online, but the write just has not landed yet"
by a single instantaneous check, so this settles for a few seconds and
asserts the server saw nothing in that window, the same way
jmap-functional's `going_back_online_flushes_a_flag_changed_while_offline`
proves the same shape at the Camel layer without a GUI).

The mock seeds the message with no keywords; the inbox starts unstarred.
"""

import time

from scenario import (
    Jmap,
    ScenarioFailure,
    expect_log,
    expect_no_errors,
    log_count,
    main,
    menu_item,
    message_is_flagged,
    say,
    screenshot,
    select_folder,
    select_message,
    wait_for,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
SUBJECT = "Welcome to jmap-mockd"
KEYWORDS_LOG = 'jmap_mail_sync: setting message keywords .*uid="{}"'
# Long enough that a write sent right away would have landed (every other
# scenario's keyword assertions settle well under this via wait_for's
# polling), short enough not to make a false negative take forever.
SETTLE_SECONDS = 5


def setup():
    write_standalone_mail_account(ACCOUNT)
    say("setup: account sources written")


def run():
    server = Jmap()
    message_id = server.emails()[SUBJECT]["id"]

    select_folder(ACCOUNT, "Inbox")
    select_message(SUBJECT)
    screenshot("01-online")
    say(f"online: folder tree and message list show {ACCOUNT!r}")

    menu_item("File/Work Offline")
    say("menu: File > Work Offline")

    select_folder(ACCOUNT, "Inbox")
    select_message(SUBJECT)
    screenshot("02-offline")
    say("offline: folder tree and message list still show the account")

    menu_item("Message/Mark As/Important")
    say("menu: Message > Mark As > Important, while offline")
    time.sleep(SETTLE_SECONDS)
    if "$flagged" in server.keywords_of(SUBJECT) or log_count(KEYWORDS_LOG.format(message_id)):
        raise ScenarioFailure(f"{SUBJECT!r} was flagged on the server while offline")
    say(f"server: {SUBJECT!r} not flagged yet, no keyword write sent while offline")
    screenshot("03-queued-offline")

    menu_item("File/Work Online")
    say("menu: File > Work Online")
    wait_for(lambda: "$flagged" in server.keywords_of(SUBJECT), f"{SUBJECT!r} flagged")
    say("server: $flagged set after reconnecting")
    expect_log(KEYWORDS_LOG.format(message_id), "the keyword write flushed on reconnect")
    wait_for(lambda: message_is_flagged(SUBJECT), f"{SUBJECT!r} shows the star icon")
    say("row: star icon shown")
    screenshot("04-flushed-online")

    expect_no_errors()


main(setup, run)
